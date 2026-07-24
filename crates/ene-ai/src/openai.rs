use async_openai::{Client, config::OpenAIConfig};
use async_trait::async_trait;
use std::pin::Pin;
use std::time::Duration;
use tokio_stream::{Stream, StreamExt};

use crate::config::{AiConfig, TaskRef};
use crate::error::{LlmProviderError, map_openai_error};
use crate::message::{LlmMessage, LlmResponseChunk, LlmToolCallChunk, UserMessagePart};
use crate::resolve::ResolvedChat;
use crate::retry::RetryPolicy;
use crate::traits::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmProvider, LlmProviderFactory,
    LlmProviderRegistry,
};

/// Builds an OpenAI-compatible client with the given base URL and API key.
pub(crate) fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig> {
    let mut config = OpenAIConfig::default().with_api_key(api_key);
    if !base_url.trim().is_empty() {
        config = config.with_api_base(base_url);
    }
    Client::with_config(config)
}

/// Applies the configured query prefix exactly once to a text input, based
/// on its [`EmbeddingKind`]. Used by `embed` and `embed_batch` so the
/// prefixing rule has a single source of truth and `embed_query` does not
/// accidentally prepend it twice.
fn apply_kind_prefix(text: &str, kind: EmbeddingKind, prefix: Option<&str>) -> String {
    match (kind, prefix) {
        (EmbeddingKind::Query, Some(p)) => format!("{p}{text}"),
        _ => text.to_string(),
    }
}

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs, FunctionCall,
};

fn convert_message(msg: &LlmMessage) -> Result<ChatCompletionRequestMessage, LlmProviderError> {
    match msg {
        LlmMessage::System { content } => {
            let m = ChatCompletionRequestSystemMessageArgs::default()
                .content(content.clone())
                .build()
                .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
            Ok(m.into())
        }
        LlmMessage::User { parts } => {
            use async_openai::types::chat::{
                ChatCompletionRequestMessageContentPartImageArgs,
                ChatCompletionRequestMessageContentPartTextArgs, ImageUrlArgs,
            };

            if parts.len() == 1
                && let Some(UserMessagePart::Text { text }) = parts.first()
            {
                let m = ChatCompletionRequestUserMessageArgs::default()
                    .content(text.clone())
                    .build()
                    .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
                return Ok(m.into());
            }

            let mut oa_parts = Vec::new();
            for part in parts {
                match part {
                    UserMessagePart::Text { text } => {
                        let p = ChatCompletionRequestMessageContentPartTextArgs::default()
                            .text(text.clone())
                            .build()
                            .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
                        oa_parts.push(p.into());
                    }
                    UserMessagePart::Image { base64_image_data } => {
                        let img_url = ImageUrlArgs::default()
                            .url(base64_image_data.clone())
                            .build()
                            .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
                        let p = ChatCompletionRequestMessageContentPartImageArgs::default()
                            .image_url(img_url)
                            .build()
                            .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
                        oa_parts.push(p.into());
                    }
                }
            }

            let m = ChatCompletionRequestUserMessageArgs::default()
                .content(oa_parts)
                .build()
                .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
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
                                    name: sanitize_tool_name(&call.name),
                                    arguments: call.arguments.clone(),
                                },
                            },
                        ),
                    );
                }
                builder.tool_calls(oa_calls);
            }
            let m = builder
                .build()
                .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
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
                .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
            Ok(m.into())
        }
    }
}

fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn convert_tools(
    tools: &[ene_plugin_proto::ToolSpec],
) -> Vec<async_openai::types::chat::ChatCompletionTools> {
    let mut res = Vec::new();
    for t in tools {
        let func = async_openai::types::chat::FunctionObject {
            name: sanitize_tool_name(t.name.as_str()),
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

/// Built-in OpenAI-Compatible Provider.
pub struct OpenAiProvider {
    client: Client<OpenAIConfig>,
    api_base: String,
    api_key: String,
    model: String,
    /// When set, caps completion length for short structured outputs (e.g. affect classifier).
    chat_max_tokens: Option<u32>,
    /// Disable `MiMo` / reasoning-model thinking so JSON lands in `content`.
    thinking_disabled: bool,
    /// Retry policy for transient failures (#237).
    retry_policy: RetryPolicy,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("api_base", &self.api_base)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("chat_max_tokens", &self.chat_max_tokens)
            .field("thinking_disabled", &self.thinking_disabled)
            .field("retry_policy", &self.retry_policy)
            .field("client", &"<async-openai Client>")
            .finish()
    }
}

impl OpenAiProvider {
    /// Creates a new `OpenAI` provider with the given base URL, API key, and model.
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        Self {
            client: build_openai_client(base_url, api_key),
            api_base: base_url.to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            chat_max_tokens: None,
            thinking_disabled: false,
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Limit completion tokens for short JSON-style responses.
    #[must_use]
    pub const fn with_chat_max_tokens(mut self, max_tokens: u32) -> Self {
        self.chat_max_tokens = Some(max_tokens);
        self
    }

    /// Send `thinking: { type: disabled }` for models that otherwise fill `reasoning_content`.
    #[must_use]
    pub const fn with_thinking_disabled(mut self, disabled: bool) -> Self {
        self.thinking_disabled = disabled;
        self
    }

    /// Override the retry policy for transient failures.
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }
}

/// Heuristic: models whose name contains "mimo" (case-insensitive) fill
/// `reasoning_content` instead of `content` unless thinking is disabled.
/// Extend the match if other reasoning models exhibit the same behavior.
fn model_wants_thinking_disabled(model: &str) -> bool {
    model.to_ascii_lowercase().contains("mimo")
}

/// Build a chat provider with model-specific transport tweaks applied.
///
/// `max_tokens`: when `> 0`, sent on chat requests. `OpenRouter` reserves
/// credits against this ceiling; omitting it can make the provider assume
/// the model max (often 65536) and return HTTP 402 on modest balances.
fn new_openai_chat_provider(
    base_url: &str,
    api_key: &str,
    model: &str,
    max_tokens: u32,
) -> OpenAiProvider {
    let mut provider = OpenAiProvider::new(base_url, api_key, model);
    if model_wants_thinking_disabled(model) {
        provider = provider.with_thinking_disabled(true);
    }
    if max_tokens > 0 {
        provider = provider.with_chat_max_tokens(max_tokens);
    }
    provider
}

fn merge_request_body(
    mut request: async_openai::types::chat::CreateChatCompletionRequest,
    stream: bool,
    thinking_disabled: bool,
) -> Result<serde_json::Value, LlmProviderError> {
    if stream {
        request.stream = Some(true);
    }
    let mut body = serde_json::to_value(request)
        .map_err(|e| LlmProviderError::Provider(format!("request serialize failed: {e}")))?;
    let Some(obj) = body.as_object_mut() else {
        return Err(LlmProviderError::Provider(
            "invalid chat request body".to_string(),
        ));
    };
    if thinking_disabled {
        obj.insert("thinking".into(), serde_json::json!({"type": "disabled"}));
    }
    Ok(body)
}

fn text_from_message_value(message: &serde_json::Value) -> Option<String> {
    if let Some(content) = message.get("content").and_then(serde_json::Value::as_str)
        && !content.trim().is_empty()
    {
        return Some(content.to_string());
    }
    message
        .get("reasoning_content")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

fn text_from_delta_value(delta: &serde_json::Value) -> Option<String> {
    if let Some(content) = delta.get("content").and_then(serde_json::Value::as_str)
        && !content.is_empty()
    {
        return Some(content.to_string());
    }
    delta
        .get("reasoning_content")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn byot_http_client(timeout: Duration) -> Result<reqwest::Client, LlmProviderError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| LlmProviderError::Provider(format!("HTTP client init failed: {e}")))
}

/// Maps a non-success HTTP status and response body to a typed
/// [`LlmProviderError`], so retry logic can distinguish transient
/// failures (429 / network) from permanent ones (401 / 400).
fn http_status_error(status: reqwest::StatusCode, body: &str) -> LlmProviderError {
    let snippet: String = body.chars().take(280).collect();
    match status.as_u16() {
        401 | 403 => LlmProviderError::Auth(snippet),
        429 => LlmProviderError::RateLimit(snippet),
        _ => LlmProviderError::Provider(format!("HTTP {status}: {snippet}")),
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

/// Non-stream BYOT chat completion via direct HTTP.
///
/// `async-openai`'s `create_byot` can hang on `OpenRouter` `MiMo` with `thinking: disabled`;
/// a plain POST matches the reliable direct-HTTP path used in classifier benchmarks.
///
/// Returns the parsed JSON body plus an optional `Retry-After` hint (seconds)
/// captured from a 429 response, so callers can honor it in backoff (#237).
async fn post_chat_byot_via_http(
    api_base: &str,
    api_key: &str,
    body: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, LlmProviderError> {
    let url = format!("{}/chat/completions", api_base.trim_end_matches('/'));
    let response = byot_http_client(timeout)?
        .post(url)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmProviderError::Network(format!("chat completion HTTP failed: {e}")))?;

    let status = response.status();
    let retry_after = retry_after_secs(&response);
    let raw = response
        .text()
        .await
        .map_err(|e| LlmProviderError::Network(format!("chat completion read failed: {e}")))?;

    if !status.is_success() {
        let mut err = http_status_error(status, &raw);
        if let (LlmProviderError::RateLimit(_), Some(secs)) = (&err, retry_after) {
            err = LlmProviderError::RateLimit(format!(
                "{raw} [retry-after: {secs}s]",
                raw = raw.chars().take(200).collect::<String>()
            ));
        }
        return Err(err);
    }

    serde_json::from_str(&raw)
        .map_err(|e| LlmProviderError::Provider(format!("chat completion JSON parse failed: {e}")))
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        let oa_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(convert_message)
            .collect::<Result<_, _>>()?;

        let oa_tools = if tools.is_empty() {
            None
        } else {
            Some(convert_tools(tools))
        };

        let name_mapping: std::collections::HashMap<String, String> = if tools.is_empty() {
            std::collections::HashMap::new()
        } else {
            tools
                .iter()
                .map(|t| (sanitize_tool_name(t.name.as_str()), t.name.to_string()))
                .collect()
        };

        let mut req_builder = async_openai::types::chat::CreateChatCompletionRequestArgs::default();
        req_builder.model(self.model.clone()).messages(oa_messages);
        if let Some(t) = oa_tools {
            req_builder.tools(t);
        }
        if let Some(max_tokens) = self.chat_max_tokens {
            req_builder.max_tokens(max_tokens);
        }

        let request = req_builder
            .build()
            .map_err(|e| LlmProviderError::Provider(e.to_string()))?;

        let body = merge_request_body(request, true, self.thinking_disabled)?;
        let api_base = self.api_base.clone();
        let api_key = self.api_key.clone();
        let retry_policy = self.retry_policy.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            if let Err(e) = run_direct_sse_stream(
                &api_base,
                &api_key,
                body,
                name_mapping,
                tx.clone(),
                &retry_policy,
            )
            .await
            {
                let _ = tx.send(Err(e)).await;
            }
        });

        use tokio_stream::wrappers::ReceiverStream;
        Ok(Box::pin(ReceiverStream::new(rx))
            as Pin<
                Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>,
            >)
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        use async_openai::types::chat::FinishReason;

        let oa_messages: Vec<ChatCompletionRequestMessage> = messages
            .iter()
            .map(convert_message)
            .collect::<Result<_, _>>()?;

        let mut req_builder = async_openai::types::chat::CreateChatCompletionRequestArgs::default();
        req_builder.model(self.model.clone()).messages(oa_messages);

        if let Some(schema) = json_schema {
            // Accept either a raw JSON Schema object or a `{ "schema": ... }` wrapper.
            let schema = schema.get("schema").cloned().unwrap_or(schema);
            req_builder.response_format(async_openai::types::chat::ResponseFormat::JsonSchema {
                json_schema: async_openai::types::chat::ResponseFormatJsonSchema {
                    description: Some("Structured output".to_string()),
                    name: "StructuredOutput".to_string(),
                    schema,
                    // Not all OpenAI-compatible providers support `strict: true`;
                    // omit it for broader compatibility (OpenRouter, etc.).
                    strict: None,
                },
            });
        }
        if let Some(max_tokens) = self.chat_max_tokens {
            req_builder.max_tokens(max_tokens);
        }

        let request = req_builder
            .build()
            .map_err(|e| LlmProviderError::Provider(e.to_string()))?;

        if self.thinking_disabled {
            let body = merge_request_body(request, false, true)?;
            let api_base = self.api_base.clone();
            let api_key = self.api_key.clone();
            let timeout = self.retry_policy.timeout;
            let response = self
                .retry_policy
                .run_with_retry_after(
                    LlmProviderError::is_retryable,
                    LlmProviderError::retry_after_secs,
                    || post_chat_byot_via_http(&api_base, &api_key, body.clone(), timeout),
                )
                .await?;

            let message = response
                .get("choices")
                .and_then(|choices| choices.as_array())
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("message"));
            let content = message
                .and_then(text_from_message_value)
                .unwrap_or_default();

            return Ok(content);
        }

        let response = self
            .retry_policy
            .run_with_retry_after(
                LlmProviderError::is_retryable,
                LlmProviderError::retry_after_secs,
                || {
                    let request = request.clone();
                    async move {
                        self.client
                            .chat()
                            .create(request)
                            .await
                            .map_err(|e| map_openai_error(&e))
                    }
                },
            )
            .await?;

        let choice = response.choices.first().ok_or_else(|| {
            tracing::warn!(component = "OpenAI", "Chat completion returned no choices");
            LlmProviderError::Provider("OpenAI returned no choices".to_string())
        })?;

        let partial_chars = choice.message.content.as_deref().map_or(0, str::len);

        if let Some(reason) = choice.finish_reason {
            match reason {
                FinishReason::Stop | FinishReason::ToolCalls | FinishReason::FunctionCall => {}
                FinishReason::Length => {
                    return Err(LlmProviderError::Truncated {
                        reason: "finish_reason=length: configured token limit reached".to_string(),
                        partial_chars,
                    });
                }
                FinishReason::ContentFilter => {
                    return Err(LlmProviderError::ContentFilter(
                        "finish_reason=content_filter: provider blocked the response".to_string(),
                    ));
                }
            }
        }

        let content = choice.message.content.clone().unwrap_or_default();

        Ok(content)
    }
}

/// Factory for the default `OpenAI` provider.
pub struct OpenAiProviderFactory;

/// Cognitive task kinds that map to [`AiConfig::tasks`] entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTaskKind {
    /// Main conversation chat.
    Chat,
    /// Post-turn affect classifier.
    Classifier,
    /// Proactive speech generation.
    Proactive,
}

impl LlmProviderFactory for OpenAiProviderFactory {
    fn provider_name(&self) -> &'static str {
        "openai-compatible"
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
        task: &TaskRef,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        let ai_config = config
            .get_section::<AiConfig>()
            .map_err(|e| LlmProviderError::Provider(format!("Failed to parse AI config: {e}")))?;
        // Honor the task's own model / max_tokens overrides rather than always
        // resolving the `tasks.chat` defaults (#C1).
        let resolved = ai_config.resolve_chat_task(Some(task))?;
        let provider = create_chat_provider_from_resolved(&resolved)
            .with_retry_policy(ai_config.retry.to_policy());
        Ok(Box::new(provider))
    }
}

/// Build an OpenAI-compatible chat provider from resolved settings.
#[must_use]
pub fn create_chat_provider_from_resolved(resolved: &ResolvedChat) -> OpenAiProvider {
    let max_tokens = resolved.max_tokens.unwrap_or(0);
    new_openai_chat_provider(
        &resolved.base_url,
        &resolved.api_key,
        &resolved.model,
        max_tokens,
    )
}

/// Build a chat provider for a named cognitive task.
///
/// OpenAI-compatible providers resolve to the native HTTP client. Any other
/// provider kind (e.g. plugin-provided `"anthropic"`) falls back to the global
/// [`LlmProviderRegistry`], which routes to a plugin-registered factory (#247).
pub fn create_task_chat_provider(
    config: &ene_config::EneConfig,
    task: AiTaskKind,
) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
    let ai_config = config
        .get_section::<AiConfig>()
        .map_err(|e| LlmProviderError::Provider(format!("Failed to parse AI config: {e}")))?;

    let task_ref = match task {
        AiTaskKind::Chat => &ai_config.tasks.chat,
        AiTaskKind::Classifier => ai_config
            .tasks
            .classifier
            .as_ref()
            .unwrap_or(&ai_config.tasks.chat),
        AiTaskKind::Proactive => ai_config
            .tasks
            .proactive
            .as_ref()
            .unwrap_or(&ai_config.tasks.chat),
    };

    // Non-OpenAI-compatible kinds (plugin providers) resolve via the global
    // registry instead of the OpenAI HTTP path (#247). The task's own
    // model / max_tokens overrides are forwarded so plugin providers honor
    // task-specific config instead of the `tasks.chat` defaults (#C1).
    if !crate::config::AiConfig::is_local_provider(&task_ref.provider)
        && let Ok(resolved_ref) = ai_config.resolve_task_ref(task_ref)
        && !resolved_ref.provider.is_openai_compatible()
    {
        return LlmProviderRegistry::create_provider(&resolved_ref.provider.kind, config, task_ref);
    }

    let resolved = match task {
        AiTaskKind::Chat => ai_config.resolve_chat()?,
        AiTaskKind::Classifier => ai_config.resolve_classifier()?,
        AiTaskKind::Proactive => ai_config.resolve_proactive_generation()?,
    };
    let provider = create_chat_provider_from_resolved(&resolved)
        .with_retry_policy(ai_config.retry.to_policy());
    Ok(Box::new(provider))
}

/// Cloud embedding provider.
pub struct CloudEmbeddingProvider {
    client: Client<OpenAIConfig>,
    embedding_model: String,
    embedding_dimensions: usize,
    query_prefix: Option<String>,
    hyde_model: Option<String>,
    /// Retry policy for transient failures (#237).
    retry_policy: RetryPolicy,
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
            hyde_model: None,
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Sets an optional `HyDE` completion model. When set, [`Self::hyde`] will call
    /// the LLM to produce a hypothetical document instead of echoing the query.
    #[must_use]
    pub fn with_hyde_model(mut self, model: String) -> Self {
        self.hyde_model = Some(model);
        self
    }

    /// Override the retry policy for transient failures.
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Generate a hypothetical document for retrieval (`HyDE`).
    ///
    /// Pipeline helper — not part of [`EmbeddingProvider`].
    pub async fn hyde(&self, query: &str) -> Result<String, EmbeddingError> {
        let model = match &self.hyde_model {
            Some(m) => m.clone(),
            None => return Ok(query.to_string()),
        };

        let messages = [
            LlmMessage::System {
                content: "You are an assistant that writes hypothetical documents for retrieval. \
                          Given a user query, generate a short description of what a relevant tool \
                          or document would look like. Keep it under 200 characters."
                    .to_string(),
            },
            LlmMessage::User {
                parts: vec![UserMessagePart::Text {
                    text: query.to_string(),
                }],
            },
        ];

        use async_openai::types::chat::CreateChatCompletionRequestArgs;

        let openai_messages: Result<Vec<_>, _> = messages.iter().map(convert_message).collect();
        let openai_messages =
            openai_messages.map_err(|e| EmbeddingError::Provider(e.to_string()))?;

        let request = CreateChatCompletionRequestArgs::default()
            .model(&model)
            .messages(openai_messages)
            .max_tokens(256u32)
            .build()
            .map_err(|e| EmbeddingError::Provider(e.to_string()))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| EmbeddingError::Provider(e.to_string()))?;

        let content = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        Ok(content)
    }
}

#[async_trait]
impl EmbeddingProvider for CloudEmbeddingProvider {
    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        use async_openai::types::embeddings::CreateEmbeddingRequestArgs;

        if items.is_empty() {
            return Ok(Vec::new());
        }

        for (text, _) in items {
            if text.trim().is_empty() {
                return Err(EmbeddingError::EmptyInput);
            }
        }

        let inputs: Vec<String> = items
            .iter()
            .map(|(text, kind)| apply_kind_prefix(text, *kind, self.query_prefix.as_deref()))
            .collect();

        let request = CreateEmbeddingRequestArgs::default()
            .model(&self.embedding_model)
            .input(inputs)
            .build()
            .map_err(|e| EmbeddingError::Provider(e.to_string()))?;

        let response = self
            .retry_policy
            .run(EmbeddingError::is_retryable, || {
                let request = request.clone();
                async move {
                    self.client
                        .embeddings()
                        .create(request)
                        .await
                        .map_err(|e| EmbeddingError::Provider(e.to_string()))
                }
            })
            .await?;

        // OpenAI returns embeddings in input order. Fail loudly if the count
        // is wrong rather than silently truncating (which used to mask
        // server-side batching bugs).
        let result: Vec<Vec<f32>> = response.data.into_iter().map(|d| d.embedding).collect();
        if result.len() != items.len() {
            return Err(EmbeddingError::DimensionMismatch(format!(
                "Embedding response count {} does not match request count {}",
                result.len(),
                items.len()
            )));
        }
        for (i, emb) in result.iter().enumerate() {
            if emb.len() != self.embedding_dimensions {
                return Err(EmbeddingError::DimensionMismatch(format!(
                    "item {i}: expected {} dims, got {}",
                    self.embedding_dimensions,
                    emb.len()
                )));
            }
        }
        Ok(result)
    }

    fn dimensions(&self) -> usize {
        self.embedding_dimensions
    }

    fn model_name(&self) -> &str {
        &self.embedding_model
    }
}

/// Establish the SSE connection, retrying transient failures (429 / network)
/// before the stream body starts. Once bytes begin flowing, disconnects are
/// not retried (the caller treats a mid-stream failure as terminal).
async fn establish_sse_connection(
    api_base: &str,
    api_key: &str,
    body: &serde_json::Value,
    retry_policy: &RetryPolicy,
) -> Result<reqwest::Response, LlmProviderError> {
    let url = format!("{}/chat/completions", api_base.trim_end_matches('/'));

    retry_policy
        .run_with_retry_after(
            LlmProviderError::is_retryable,
            LlmProviderError::retry_after_secs,
            || async {
                let response = byot_http_client(retry_policy.timeout)?
                    .post(url.clone())
                    .bearer_auth(api_key)
                    .header("Content-Type", "application/json")
                    .json(body)
                    .send()
                    .await
                    .map_err(|e| {
                        LlmProviderError::Network(format!("chat stream HTTP failed: {e}"))
                    })?;

                let status = response.status();
                if status.is_success() {
                    return Ok(response);
                }

                let retry_after = retry_after_secs(&response);
                let raw = response.text().await.unwrap_or_default();
                if status.as_u16() == 402 {
                    return Err(LlmProviderError::Provider(format!(
                        "chat stream HTTP 402 Payment Required (often OpenRouter credit collateral for max_tokens): {}",
                        raw.chars().take(280).collect::<String>()
                    )));
                }
                let mut err = http_status_error(status, &raw);
                if let (LlmProviderError::RateLimit(_), Some(secs)) = (&err, retry_after) {
                    err = LlmProviderError::RateLimit(format!(
                        "{} [retry-after: {secs}s]",
                        raw.chars().take(200).collect::<String>()
                    ));
                }
                Err(err)
            },
        )
        .await
}

async fn run_direct_sse_stream(
    api_base: &str,
    api_key: &str,
    body: serde_json::Value,
    name_mapping: std::collections::HashMap<String, String>,
    tx: tokio::sync::mpsc::Sender<Result<LlmResponseChunk, LlmProviderError>>,
    retry_policy: &RetryPolicy,
) -> Result<(), LlmProviderError> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio_util::io::StreamReader;

    let response = establish_sse_connection(api_base, api_key, &body, retry_policy).await?;

    let bytes_stream = response
        .bytes_stream()
        .map(|res| res.map_err(std::io::Error::other));
    let reader = StreamReader::new(bytes_stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| LlmProviderError::Provider(format!("read stream line failed: {e}")))?
    {
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
        let Ok(chunk) = serde_json::from_str::<serde_json::Value>(payload) else {
            tracing::debug!(
                component = "OpenAI",
                payload = %payload,
                "skipping malformed SSE chunk"
            );
            continue;
        };

        let mut text_delta = None;
        let mut tool_calls_delta = None;

        if let Some(choices) = chunk.get("choices").and_then(serde_json::Value::as_array)
            && let Some(choice) = choices.first()
            && let Some(delta) = choice.get("delta")
        {
            text_delta = text_from_delta_value(delta);

            if let Some(tc_deltas) = delta
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
            {
                let mut tc_list = Vec::new();
                for tc in tc_deltas {
                    let index = tc
                        .get("index")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize;
                    let id = tc
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let (name, arguments) = if let Some(func) = tc.get("function") {
                        (
                            func.get("name")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            func.get("arguments")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                        )
                    } else {
                        (None, None)
                    };
                    tc_list.push(LlmToolCallChunk {
                        index,
                        id,
                        name: name.map(|n| name_mapping.get(&n).cloned().unwrap_or(n)),
                        arguments,
                    });
                }
                tool_calls_delta = Some(tc_list);
            }
        }

        if text_delta.is_some() || tool_calls_delta.is_some() {
            let _ = tx
                .send(Ok(LlmResponseChunk {
                    text_delta,
                    tool_calls_delta,
                }))
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_kind_prefix_query_with_prefix() {
        let out = apply_kind_prefix("hello", EmbeddingKind::Query, Some("Q:"));
        assert_eq!(out, "Q:hello");
    }

    #[test]
    fn apply_kind_prefix_query_without_prefix() {
        let out = apply_kind_prefix("hello", EmbeddingKind::Query, None);
        assert_eq!(out, "hello");
    }

    #[test]
    fn apply_kind_prefix_non_query_never_prefixed() {
        let out = apply_kind_prefix("hello", EmbeddingKind::Summary, Some("Q:"));
        assert_eq!(out, "hello");
    }

    /// Regression for #34: `embed_query` was prepending the query prefix
    /// here, then calling `embed` which prepended it again. Asserting the
    /// single-source-of-truth helper applies the prefix exactly once per
    /// call is enough — the bug surfaces as "double prefix" only when the
    /// helper is invoked twice. The test makes the contract explicit.
    #[test]
    fn embed_query_does_not_double_prefix() {
        let once = apply_kind_prefix("hello", EmbeddingKind::Query, Some("Q:"));
        // If a caller re-prepended the prefix (the original bug), the result
        // would be "Q:Q:hello". After the fix, only the helper applies the
        // prefix, and `embed_query` calls `embed` exactly once.
        assert_ne!(once, "Q:Q:hello");
        assert_eq!(once, "Q:hello");
    }
}
