//! # ene-provider
//!
//! Unified LLM provider abstraction layer for the ene AI character platform.
//!
//! Defines generic message and streaming types (`LlmMessage`, `LlmResponseChunk`),
//! provider traits (`LlmProvider`, `EmbeddingProvider`, `LlmProviderFactory`),
//! a global provider registry, and the built-in OpenAI-compatible implementation.
#![warn(missing_docs)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use tokio_stream::{Stream, StreamExt};

/// Unified chat message formats for LLM providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "role")]
pub enum LlmMessage {
    /// System instruction message.
    System {
        /// The system prompt content.
        content: String,
    },
    /// User prompt message, supporting multimodal text and image parts.
    User {
        /// The list of content parts (text and/or images).
        parts: Vec<UserMessagePart>,
    },
    /// Assistant reply, potentially including content and/or tool executions.
    Assistant {
        /// The text content of the assistant's reply, if any.
        content: Option<String>,
        /// Tool calls initiated by the model, if any.
        tool_calls: Option<Vec<LlmToolCall>>,
    },
    /// Response from a executed tool.
    Tool {
        /// The ID of the tool call this response is for.
        tool_call_id: String,
        /// The tool execution result content.
        content: String,
    },
}

/// Unified user prompt content part (multimodal support).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserMessagePart {
    /// A block of text.
    Text {
        /// The text content.
        text: String,
    },
    /// A base64-encoded screenshot or image.
    Image {
        /// Base64-encoded image data (with data URI prefix).
        base64_image_data: String,
    },
}

/// Generic representation of a tool call initiated by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmToolCall {
    /// Unique identifier for this specific tool execution request.
    pub id: String,
    /// The name of the tool to invoke.
    pub name: String,
    /// Arguments for the tool in JSON-serialized format.
    pub arguments: String,
}

/// Generic representation of a streaming response chunk.
#[derive(Debug, Clone)]
pub struct LlmResponseChunk {
    /// Text delta generated in this chunk, if any.
    pub text_delta: Option<String>,
    /// Tool call updates generated in this chunk, if any.
    pub tool_calls_delta: Option<Vec<LlmToolCallChunk>>,
}

/// Generic representation of a tool call fragment in a stream.
#[derive(Debug, Clone)]
pub struct LlmToolCallChunk {
    /// The index of the tool call in the array.
    pub index: usize,
    /// Optional tool call ID.
    pub id: Option<String>,
    /// Optional tool name delta.
    pub name: Option<String>,
    /// Optional tool arguments delta.
    pub arguments: Option<String>,
}

/// Trait implemented by LLM providers to interface with Ene.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Returns the unique name of this provider.
    fn name(&self) -> &str;

    /// Initiates a chat completion stream with the given messages and tools.
    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_tool_proto::ToolDefinition],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, String>> + Send>>, String>;

    /// Executes a non-streaming chat completion with optional JSON schema response.
    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, String>;
}

/// Trait for generating vector embeddings from text (used by memory search).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding vector for the given text (used for indexing / storage).
    async fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
    /// Generate an embedding vector optimised for query / search.
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, String>;
    /// The dimensionality of the embedding vectors produced by this provider.
    fn dimensions(&self) -> usize;
    /// A human-readable name identifying the embedding model.
    fn model_name(&self) -> &str;
}

/// Factory trait to build specific `LlmProvider` instances from workspace configs.
pub trait LlmProviderFactory: Send + Sync {
    /// The unique name of the provider this factory produces.
    fn provider_name(&self) -> &str;

    /// Instantiates the provider based on current `EneConfig` settings.
    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, String>;
}

/// Global registry of `LlmProviderFactory` implementations.
pub struct LlmProviderRegistry {
    factories: Mutex<HashMap<String, Arc<dyn LlmProviderFactory>>>,
}

impl LlmProviderRegistry {
    /// Returns the static, thread-safe global registry instance.
    fn global() -> &'static Self {
        static REGISTRY: OnceLock<LlmProviderRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| Self {
            factories: Mutex::new(HashMap::new()),
        })
    }

    /// Registers a new provider factory.
    pub fn register(factory: Arc<dyn LlmProviderFactory>) {
        let name = factory.provider_name().to_string();
        if let Ok(mut guard) = Self::global().factories.lock() {
            guard.insert(name, factory);
        }
    }

    /// Tries to instantiate a provider by name using the registered factories.
    pub fn create_provider(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, String> {
        let factory = {
            if let Ok(guard) = Self::global().factories.lock() {
                guard.get(name).cloned()
            } else {
                None
            }
        };

        match factory {
            Some(f) => f.create_provider(config),
            None => Err(format!(
                "No LlmProviderFactory registered for provider name: '{}'",
                name
            )),
        }
    }
}

// =========================================================================
// Built-in Config definition (formerly in ene-core)
// =========================================================================

fn default_string() -> String {
    String::new()
}

/// Configuration for the local GGUF embedding backend.
#[derive(Debug, Clone, ene_config::serde::Serialize, ene_config::serde::Deserialize)]
pub struct LocalEmbeddingConfig {
    /// Local GGUF embedding model name (e.g. `"jina-embeddings-v5-text-small"`).
    #[serde(default = "LocalEmbeddingConfig::default_model")]
    pub model: String,
    /// Quantization level (e.g. `"F16"`, `"Q4_K_M"`).
    #[serde(default = "LocalEmbeddingConfig::default_quantization")]
    pub quantization: String,
}

impl LocalEmbeddingConfig {
    fn default_model() -> String {
        "jina-embeddings-v5-text-small".to_string()
    }
    fn default_quantization() -> String {
        "F16".to_string()
    }
}

impl Default for LocalEmbeddingConfig {
    fn default() -> Self {
        Self {
            model: Self::default_model(),
            quantization: Self::default_quantization(),
        }
    }
}

ene_config::define_config!(
    "provider",
    /// AI provider connection config, including embedding backend settings.
    pub struct ProviderConfig {
        /// Provider name (e.g. `"openai-compatible"`).
        pub provider_name: String = "openai-compatible".to_string(),
        /// Chat model name (e.g. `"gpt-4o-mini"`).
        pub model: String = "gpt-4o-mini".to_string(),
        /// API base URL.
        pub base_url: String = default_string(),
        /// API key (inline — use with caution).
        pub api_key: String = default_string(),
        /// Key source: `"inline"`, `"env"`, or `"keyring"`.
        pub api_key_source: String = "inline".to_string(),
        /// Environment variable name when `api_key_source = "env"`.
        pub api_key_env: String = "OPENAI_API_KEY".to_string(),
        /// Keyring service name when `api_key_source = "keyring"`.
        pub api_key_keyring_service: String = "dev.pexisgle.ene".to_string(),
        /// Keyring account name when `api_key_source = "keyring"`.
        pub api_key_keyring_account: String = "default".to_string(),
        /// Embedding backend: `"cloud"` uses the same provider's embedding API;
        /// `"local"` uses a local GGUF model via `ene-embedding`.
        pub embedding_backend: String = "cloud".to_string(),
        /// Cloud embedding model name (used when `embedding_backend = "cloud"`).
        pub cloud_embedding_model: String = "text-embedding-3-small".to_string(),
        /// Expected dimensions for cloud embedding vectors.
        pub cloud_embedding_dimensions: usize = 1536,
        /// Optional query prefix to prepend to search queries (e.g. "Query: ").
        pub query_prefix: Option<String> = None,
    }
);

impl ProviderConfig {
    /// Resolves the effective base URL, falling back to defaults.
    pub fn resolve_base_url(&self) -> Result<String, ene_config::ConfigError> {
        if !self.base_url.trim().is_empty() {
            return Ok(self.base_url.clone());
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: String::new(),
        })
    }

    /// Resolves the API key from the configured source (inline, env, or keyring).
    pub fn resolve_api_key(&self) -> String {
        match self.api_key_source.as_str() {
            "inline" => {
                if !self.api_key.trim().is_empty() {
                    return self.api_key.clone();
                }
                #[cfg(debug_assertions)]
                {
                    if let Ok(token) = std::env::var("API_TOKEN") {
                        if !token.trim().is_empty() {
                            return token;
                        }
                    }
                }
                String::new()
            }
            "env" => {
                let var_name = if self.api_key_env.trim().is_empty() {
                    "OPENAI_API_KEY"
                } else {
                    self.api_key_env.trim()
                };
                std::env::var(var_name).unwrap_or_default()
            }
            "keyring" => {
                tracing::warn!(
                    "Keyring support is temporarily disabled. Please use 'inline' or 'env' source."
                );
                String::new()
            }
            _ => {
                if !self.api_key.trim().is_empty() {
                    return self.api_key.clone();
                }
                #[cfg(debug_assertions)]
                {
                    if let Ok(token) = std::env::var("API_TOKEN") {
                        if !token.trim().is_empty() {
                            return token;
                        }
                    }
                }
                String::new()
            }
        }
    }

    /// Reads the `local_embedding` sub-section from config.
    ///
    /// Falls back to `LocalEmbeddingConfig::default()` if the key is absent.
    pub fn local_embedding(config: &ene_config::EneConfig) -> LocalEmbeddingConfig {
        config
            .get_section_by_key::<LocalEmbeddingConfig>("provider.local_embedding")
            .unwrap_or_default()
    }
}

// =========================================================================
// Built-in OpenAI-Compatible Provider Implementation
// =========================================================================

use async_openai::{Client, config::OpenAIConfig};

/// Builds an OpenAI-compatible client with the given base URL and API key.
pub(crate) fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig> {
    let mut config = OpenAIConfig::default().with_api_key(api_key);
    if !base_url.trim().is_empty() {
        config = config.with_api_base(base_url);
    }
    Client::with_config(config)
}

/// Built-in OpenAI-Compatible Provider.
pub struct OpenAiProvider {
    client: Client<OpenAIConfig>,
    model: String,
}

impl OpenAiProvider {
    /// Creates a new OpenAI provider with the given base URL, API key, and model.
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        _embedding_model: &str,
        _embedding_dimensions: usize,
    ) -> Self {
        Self {
            client: build_openai_client(base_url, api_key),
            model: model.to_string(),
        }
    }
}

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs, FunctionCall,
};

fn convert_message(msg: &LlmMessage) -> Result<ChatCompletionRequestMessage, String> {
    match msg {
        LlmMessage::System { content } => {
            let m = ChatCompletionRequestSystemMessageArgs::default()
                .content(content.clone())
                .build()
                .map_err(|e| e.to_string())?;
            Ok(m.into())
        }
        LlmMessage::User { parts } => {
            use async_openai::types::chat::{
                ChatCompletionRequestMessageContentPartImageArgs,
                ChatCompletionRequestMessageContentPartTextArgs, ImageUrlArgs,
            };

            if parts.len() == 1 {
                if let UserMessagePart::Text { text } = &parts[0] {
                    let m = ChatCompletionRequestUserMessageArgs::default()
                        .content(text.clone())
                        .build()
                        .map_err(|e| e.to_string())?;
                    return Ok(m.into());
                }
            }

            let mut oa_parts = Vec::new();
            for part in parts {
                match part {
                    UserMessagePart::Text { text } => {
                        let p = ChatCompletionRequestMessageContentPartTextArgs::default()
                            .text(text.clone())
                            .build()
                            .map_err(|e| e.to_string())?;
                        oa_parts.push(p.into());
                    }
                    UserMessagePart::Image { base64_image_data } => {
                        let img_url = ImageUrlArgs::default()
                            .url(base64_image_data.clone())
                            .build()
                            .map_err(|e| e.to_string())?;
                        let p = ChatCompletionRequestMessageContentPartImageArgs::default()
                            .image_url(img_url)
                            .build()
                            .map_err(|e| e.to_string())?;
                        oa_parts.push(p.into());
                    }
                }
            }

            let m = ChatCompletionRequestUserMessageArgs::default()
                .content(oa_parts)
                .build()
                .map_err(|e| e.to_string())?;
            Ok(m.into())
        }
        LlmMessage::Assistant {
            content,
            tool_calls,
        } => {
            let mut builder = ChatCompletionRequestAssistantMessageArgs::default();
            if let Some(c) = content {
                builder.content(c.clone());
            }
            if let Some(calls) = tool_calls {
                let mut oa_calls = Vec::new();
                for call in calls {
                    oa_calls.push(
                        async_openai::types::chat::ChatCompletionMessageToolCalls::Function(
                            ChatCompletionMessageToolCall {
                                id: call.id.clone(),
                                function: FunctionCall {
                                    name: call.name.clone(),
                                    arguments: call.arguments.clone(),
                                },
                            },
                        ),
                    );
                }
                builder.tool_calls(oa_calls);
            }
            let m = builder.build().map_err(|e| e.to_string())?;
            Ok(m.into())
        }
        LlmMessage::Tool {
            tool_call_id,
            content,
        } => {
            let m = ChatCompletionRequestToolMessageArgs::default()
                .tool_call_id(tool_call_id.clone())
                .content(content.clone())
                .build()
                .map_err(|e| e.to_string())?;
            Ok(m.into())
        }
    }
}

fn convert_tools(
    tools: &[ene_tool_proto::ToolDefinition],
) -> Vec<async_openai::types::chat::ChatCompletionTools> {
    let mut res = Vec::new();
    for t in tools {
        let func = async_openai::types::chat::FunctionObject {
            name: t.name.clone(),
            description: Some(t.description.clone()),
            parameters: Some(t.parameters.clone()),
            strict: None,
        };
        res.push(async_openai::types::chat::ChatCompletionTools::Function(
            async_openai::types::chat::ChatCompletionTool { function: func },
        ));
    }
    res
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_tool_proto::ToolDefinition],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, String>> + Send>>, String> {
        let oa_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(convert_message)
            .collect::<Result<_, _>>()?;

        let oa_tools = if !tools.is_empty() {
            Some(convert_tools(tools))
        } else {
            None
        };

        let mut req_builder = async_openai::types::chat::CreateChatCompletionRequestArgs::default();
        req_builder.model(self.model.clone()).messages(oa_messages);
        if let Some(t) = oa_tools {
            req_builder.tools(t);
        }

        let request = req_builder.build().map_err(|e| e.to_string())?;
        let stream = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|e| e.to_string())?;

        let mapped_stream = stream.map(|chunk_res| match chunk_res {
            Ok(chunk) => {
                let mut text_delta = None;
                let mut tool_calls_delta = None;

                if let Some(choice) = chunk.choices.first() {
                    if let Some(content) = &choice.delta.content {
                        text_delta = Some(content.clone());
                    }

                    if let Some(tc_deltas) = &choice.delta.tool_calls {
                        let mut tc_list = Vec::new();
                        for tc in tc_deltas {
                            let mut name = None;
                            let mut arguments = None;
                            if let Some(func) = &tc.function {
                                if let Some(n) = &func.name {
                                    name = Some(n.clone());
                                }
                                if let Some(a) = &func.arguments {
                                    arguments = Some(a.clone());
                                }
                            }
                            tc_list.push(LlmToolCallChunk {
                                index: tc.index as usize,
                                id: tc.id.clone(),
                                name,
                                arguments,
                            });
                        }
                        tool_calls_delta = Some(tc_list);
                    }
                }

                Ok(LlmResponseChunk {
                    text_delta,
                    tool_calls_delta,
                })
            }
            Err(e) => Err(e.to_string()),
        });

        Ok(Box::pin(mapped_stream)
            as Pin<
                Box<dyn Stream<Item = Result<LlmResponseChunk, String>> + Send>,
            >)
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, String> {
        let oa_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(convert_message)
            .collect::<Result<_, _>>()?;

        let mut req_builder = async_openai::types::chat::CreateChatCompletionRequestArgs::default();
        req_builder.model(self.model.clone()).messages(oa_messages);

        if let Some(schema) = json_schema {
            req_builder.response_format(async_openai::types::chat::ResponseFormat::JsonSchema {
                json_schema: async_openai::types::chat::ResponseFormatJsonSchema {
                    description: Some("Structured output".to_string()),
                    name: "StructuredOutput".to_string(),
                    schema,
                    strict: Some(true),
                },
            });
        }

        let request = req_builder.build().map_err(|e| e.to_string())?;
        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| e.to_string())?;

        let content = response
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .unwrap_or("")
            .to_string();

        Ok(content)
    }
}

/// Factory for the default OpenAI provider.
pub struct OpenAiProviderFactory;

impl LlmProviderFactory for OpenAiProviderFactory {
    fn provider_name(&self) -> &str {
        "openai-compatible"
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn LlmProvider>, String> {
        let provider_config = config
            .get_section::<ProviderConfig>()
            .map_err(|e| format!("Failed to parse provider config: {}", e))?;

        let base_url = provider_config
            .resolve_base_url()
            .map_err(|e| format!("Failed to resolve base URL: {}", e))?;

        let api_key = provider_config.resolve_api_key();

        Ok(Box::new(OpenAiProvider::new(
            &base_url,
            &api_key,
            &provider_config.model,
            &provider_config.cloud_embedding_model,
            provider_config.cloud_embedding_dimensions,
        )))
    }
}

/// Cloud embedding provider.
pub struct CloudEmbeddingProvider {
    client: Client<OpenAIConfig>,
    embedding_model: String,
    embedding_dimensions: usize,
    query_prefix: Option<String>,
}

impl CloudEmbeddingProvider {
    /// Creates a new `CloudEmbeddingProvider`.
    pub fn new(
        base_url: &str,
        api_key: &str,
        embedding_model: &str,
        embedding_dimensions: usize,
        query_prefix: Option<String>,
    ) -> Self {
        Self {
            client: build_openai_client(base_url, api_key),
            embedding_model: embedding_model.to_string(),
            embedding_dimensions,
            query_prefix,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for CloudEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        use async_openai::types::embeddings::CreateEmbeddingRequestArgs;
        let request = CreateEmbeddingRequestArgs::default()
            .model(&self.embedding_model)
            .input(text)
            .build()
            .map_err(|e| e.to_string())?;

        let response = self
            .client
            .embeddings()
            .create(request)
            .await
            .map_err(|e| e.to_string())?;
        let embedding = response
            .data
            .into_iter()
            .next()
            .ok_or_else(|| "Empty embedding response".to_string())?;
        Ok(embedding.embedding)
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, String> {
        if let Some(prefix) = &self.query_prefix {
            let prefixed = format!("{}{}", prefix, text);
            self.embed(&prefixed).await
        } else {
            self.embed(text).await
        }
    }

    fn dimensions(&self) -> usize {
        self.embedding_dimensions
    }

    fn model_name(&self) -> &str {
        &self.embedding_model
    }
}
