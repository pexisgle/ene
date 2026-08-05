//! Resolved provider settings from [`AiConfig`] task routing.

use crate::config::{
    AiConfig, AiProviderDef, ApiKeyConfig, LOCAL_PROVIDER, LocalModelDef, TaskRef,
    kind_typo_suggestion,
};
use crate::error::LlmProviderError;
use crate::message::{LlmMessage, UserMessagePart};
use crate::traits::{LlmProvider, ProviderHost};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Fully resolved OpenAI-compatible chat settings for a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChat {
    /// Effective API base URL.
    pub base_url: String,
    /// Resolved API key.
    pub api_key: String,
    /// Model name (required for cloud chat workloads).
    pub model: String,
    /// Optional completion token cap (`None` = omit on requests).
    pub max_tokens: Option<u32>,
}

/// Fully resolved local GGUF model settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLocalModel {
    /// Registry key in [`AiConfig::local_models`].
    pub name: String,
    /// HTTPS download URL.
    pub url: String,
    /// Optional filesystem path override.
    pub model_path: String,
    /// Optional multimodal projector download URL.
    pub mmproj_url: String,
    /// Optional multimodal projector filesystem path.
    pub mmproj_path: String,
    /// Quantization label.
    pub quantization: String,
    /// Preferred acceleration backend.
    pub acceleration: crate::config::ProactiveAcceleration,
    /// GPU layer offload setting.
    pub gpu_layers: String,
    /// Context size for decision workloads.
    pub context_size: u32,
    /// Real embedding dimensionality, when the model backs `tasks.embedding`
    /// (see [`LocalModelDef::dimensions`]).
    pub dimensions: Option<usize>,
}

impl ResolvedLocalModel {
    pub(crate) fn from_named(name: &str, def: &LocalModelDef) -> Self {
        // #313: `mmproj_url` / `mmproj_path` / `acceleration` moved out of
        // `LocalModelDef` into the llama.cpp plugin config
        // (`plugins.list.llama-cpp.config`). Until the llama.cpp provider
        // plugin exists, the in-process readers keep working by sourcing them
        // from the plugin config here at resolve time.
        let llama_cpp = crate::plugin_config::LlamaCppPluginConfig::global();
        Self {
            name: name.to_string(),
            url: def.url.clone(),
            model_path: def.model_path.clone(),
            mmproj_url: llama_cpp.mmproj_url,
            mmproj_path: llama_cpp.mmproj_path,
            quantization: def.quantization.clone(),
            acceleration: llama_cpp.acceleration,
            gpu_layers: def.gpu_layers.clone(),
            context_size: def.context_size,
            dimensions: def.dimensions,
        }
    }

    /// True when an mmproj URL or path is configured.
    #[must_use]
    pub fn has_mmproj(&self) -> bool {
        !self.mmproj_path.trim().is_empty() || !self.mmproj_url.trim().is_empty()
    }
}

/// Fully resolved embedding backend settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedEmbedding {
    /// Cloud embedding via OpenAI-compatible API.
    Cloud {
        /// Canonical provider kind (the plugin registry key, e.g. `"openai"`).
        kind: String,
        /// API base URL.
        base_url: String,
        /// Resolved API key.
        api_key: String,
        /// Embedding model name.
        model: String,
        /// Expected vector dimensions.
        dimensions: usize,
        /// Optional query prefix for retrieval queries.
        query_prefix: Option<String>,
    },
    /// Local GGUF embedding via llama-cpp-4.
    Local(ResolvedLocalModel),
}

impl ResolvedEmbedding {
    /// Cloud embedding fields, or `None` if this is a local embedding.
    ///
    /// `(kind, base_url, api_key, model, dimensions, query_prefix)`.
    #[must_use]
    #[expect(
        clippy::type_complexity,
        reason = "fixed-arity tuple mirrors the ResolvedEmbedding::Cloud fields; consumers destructure it positionally"
    )]
    pub fn cloud_fields(&self) -> Option<(&str, &str, &str, &str, usize, Option<&str>)> {
        match self {
            Self::Cloud {
                kind,
                base_url,
                api_key,
                model,
                dimensions,
                query_prefix,
            } => Some((
                kind.as_str(),
                base_url.as_str(),
                api_key.as_str(),
                model.as_str(),
                *dimensions,
                query_prefix.as_deref(),
            )),
            Self::Local(_) => None,
        }
    }

    /// Local embedding fields, or `None` if this is a cloud embedding.
    #[must_use]
    pub fn local_fields(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::Local(local) => Some((
                local.name.as_str(),
                local.quantization.as_str(),
                local.url.as_str(),
            )),
            Self::Cloud { .. } => None,
        }
    }

    /// Resolved local model reference.
    #[must_use]
    pub fn as_local(&self) -> Option<&ResolvedLocalModel> {
        match self {
            Self::Local(local) => Some(local),
            Self::Cloud { .. } => None,
        }
    }
}

/// Fully resolved TTS provider settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTts {
    /// Provider name (e.g. `"kokoro"`, `"openai"`).
    pub provider: String,
    /// Model name (provider-specific).
    pub model: String,
    /// Voice identifier, if configured.
    pub voice: Option<String>,
    /// Speech speed multiplier (1.0 = normal), clamped to `[0.1, 5.0]`.
    pub speed: f32,
    /// Language code for G2P (e.g. `"en"`, `"ja"`), if configured.
    pub language: Option<String>,
}

/// Fully resolved STT provider settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStt {
    /// Provider name (e.g. `"whisper"`, `"openai"`).
    pub provider: String,
    /// Model name (provider-specific).
    pub model: String,
    /// Language hint, if configured.
    pub language: Option<String>,
}

/// Fully resolved VAD engine settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedVad {
    /// Engine name (e.g. `"silero"`, `"webrtc"`).
    pub provider: String,
}

/// Resolved task reference: provider definition plus per-task overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTaskRef<'a> {
    /// Provider definition.
    pub provider: &'a AiProviderDef,
    /// Task model override.
    pub model: Option<&'a str>,
    /// Task max-tokens override.
    pub max_tokens: Option<u32>,
    /// Task embedding dimensions override.
    pub dimensions: Option<usize>,
}

/// A fully resolved cloud chat candidate for failover routing.
///
/// Produced by [`AiConfig::resolve_chat_candidates`], which enumerates the
/// configured chat provider first (highest priority) followed by every other
/// cloud provider in [`AiConfig::providers`] order. The runtime probes each
/// candidate's health and selects the first available one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCandidate {
    /// Provider name (key in [`AiConfig::providers`]).
    pub provider: String,
    /// Canonical provider kind (the plugin registry key, e.g. `"openai"`).
    pub kind: String,
    /// Effective API base URL.
    pub base_url: String,
    /// Resolved API key.
    pub api_key: String,
    /// Model name.
    pub model: String,
    /// Optional completion token cap (`None` = omit on requests).
    pub max_tokens: Option<u32>,
}

impl ChatCandidate {
    /// Convert to the resolved chat settings used to build a provider.
    #[must_use]
    pub fn to_resolved(&self) -> ResolvedChat {
        ResolvedChat {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            max_tokens: self.max_tokens,
        }
    }
}

/// Resolves an explicit or env-provided base URL for OpenAI-compatible APIs.
pub fn resolve_base_url(base_url: &str) -> Result<String, LlmProviderError> {
    if !base_url.trim().is_empty() {
        return Ok(base_url.to_string());
    }
    if let Ok(url) = std::env::var("OPENAI_BASE_URL")
        && !url.trim().is_empty()
    {
        return Ok(url);
    }
    Err(LlmProviderError::Provider(
        "base URL not configured; set base_url or OPENAI_BASE_URL".to_string(),
    ))
}

impl ApiKeyConfig {
    /// Resolves the API key from the configured source (inline or env).
    #[must_use]
    pub fn resolve_api_key(&self) -> String {
        if self.source.as_str() == "env" {
            let var_name = if self.env.trim().is_empty() {
                "OPENAI_API_KEY"
            } else {
                self.env.trim()
            };
            std::env::var(var_name).unwrap_or_default()
        } else if !self.inline.trim().is_empty() {
            self.inline.clone()
        } else {
            String::new()
        }
    }
}

/// A single settings validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsIssue {
    /// Chat provider API key resolves empty.
    MissingApiKey {
        /// Provider key in `ai.providers`.
        provider: String,
    },
    /// Chat provider base URL is empty.
    MissingBaseUrl {
        /// Provider key in `ai.providers`.
        provider: String,
    },
    /// Chat provider base URL is not an http(s) URL.
    InvalidBaseUrl {
        /// Provider key in `ai.providers`.
        provider: String,
        /// Human-readable detail.
        detail: String,
    },
    /// Provider `kind` looks like a typo of a built-in kind.
    SuspiciousKind {
        /// Provider key in `ai.providers`.
        provider: String,
        /// The configured (suspect) kind value.
        kind: String,
        /// Suggested built-in kind, if any.
        suggestion: Option<String>,
    },
}

impl SettingsIssue {
    /// Stable English message for CLI / UI.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::MissingApiKey { provider } => {
                format!("API key not set for provider `{provider}`")
            }
            Self::MissingBaseUrl { provider } => {
                format!("Base URL not set for provider `{provider}`")
            }
            Self::InvalidBaseUrl { provider, detail } => {
                format!("Invalid base URL for provider `{provider}`: {detail}")
            }
            Self::SuspiciousKind {
                provider,
                kind,
                suggestion,
            } => {
                let hint = suggestion
                    .as_ref()
                    .map_or_else(String::new, |s| format!(" (did you mean `{s}`?)"));
                format!("Unknown provider kind `{kind}` for provider `{provider}`{hint}")
            }
        }
    }
}

/// Validate provider `kind` values against known built-ins.
///
/// Catches obvious typos of built-in kinds (e.g. `"openai-compatible"` or
/// `"anthroic"`) at config load / resolve time instead of failing late and
/// quietly. The check is lenient toward plugin-provided kinds: any kind that
/// is not a near-miss of a built-in is assumed to name a plugin backend and is
/// left alone, since the full set of plugin kinds is only known at runtime.
#[must_use]
pub fn validate_provider_kinds(ai: &AiConfig) -> Vec<SettingsIssue> {
    let mut issues = Vec::new();
    for (name, def) in &ai.providers {
        if let Some(suggestion) = kind_typo_suggestion(&def.kind) {
            issues.push(SettingsIssue::SuspiciousKind {
                provider: name.clone(),
                kind: def.kind.clone(),
                suggestion: Some(suggestion.to_string()),
            });
        }
    }
    issues
}

/// Validate chat-provider settings without performing a network call.
///
/// Checks that the configured chat provider has a non-empty base URL with an
/// `http`/`https` scheme and a resolvable API key. Returns an empty vec when
/// the section is missing or the chat provider is local/GGUF. Also reports
/// provider `kind` values that look like typos of built-in kinds.
#[must_use]
pub fn validate_settings(config: &ene_config::EneConfig) -> Vec<SettingsIssue> {
    let Ok(ai) = config.get_section::<crate::AiConfig>() else {
        return Vec::new();
    };

    // Validate every provider's `kind` up front, independent of which provider
    // the chat task routes to.
    let mut issues = validate_provider_kinds(&ai);

    let provider_key = ai.tasks.chat.provider.clone();
    if crate::AiConfig::is_local_provider(&provider_key) {
        return issues;
    }
    let Some(def) = ai.providers.get(&provider_key) else {
        issues.push(SettingsIssue::MissingBaseUrl {
            provider: provider_key,
        });
        return issues;
    };
    // base_url / api_key are only meaningful for OpenAI-compatible HTTP
    // providers; plugin-provided kinds validate over IPC instead.
    if !def.is_openai_compatible() {
        return issues;
    }

    let url = def.base_url.trim();
    if url.is_empty() {
        issues.push(SettingsIssue::MissingBaseUrl {
            provider: provider_key.clone(),
        });
    } else if !(url.starts_with("http://") || url.starts_with("https://")) {
        issues.push(SettingsIssue::InvalidBaseUrl {
            provider: provider_key.clone(),
            detail: "must start with http:// or https://".to_string(),
        });
    }
    if def.api_key.resolve_api_key().trim().is_empty() {
        issues.push(SettingsIssue::MissingApiKey {
            provider: provider_key,
        });
    }
    issues
}

/// Whether chat settings look incomplete enough to warrant first-run onboarding.
#[must_use]
pub fn needs_onboarding(config: &ene_config::EneConfig) -> bool {
    validate_settings(config)
        .iter()
        .any(|i| matches!(i, SettingsIssue::MissingApiKey { .. }))
}

/// A task whose configured context window cannot hold its prompt budget plus
/// output reserve.
///
/// Produced by [`validate_context_budgets`] and surfaced as a startup warning:
/// when a model's window is smaller than what the prompt composition needs,
/// sections are silently dropped every turn. The struct carries the full
/// breakdown so the caller can build a precise diagnostic message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetIssue {
    /// Task name (e.g. `"chat"`, `"proactive"`).
    pub task: String,
    /// Local model registry key, or the cloud provider name.
    pub model: String,
    /// Effective context window in tokens (`min(advertised, configured)`).
    pub effective_window: u32,
    /// Prompt budget the task composes against, in tokens.
    pub prompt_budget: u32,
    /// Tokens reserved for the model's reply (`tasks.<task>.max_tokens`).
    pub response_reserve: u32,
    /// `prompt_budget + response_reserve` — what the window must hold.
    pub required: u32,
}

impl ContextBudgetIssue {
    /// Stable English message for CLI / UI / logs.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "model '{}' for task '{}' has an effective context window of {} tokens but the current config requires {} ({} prompt + {} response); prompt sections will be dropped every turn",
            self.model,
            self.task,
            self.effective_window,
            self.required,
            self.prompt_budget,
            self.response_reserve
        )
    }
}

/// Validate that each generative task's context window can hold its prompt
/// budget plus output reserve.
///
/// `prompt_budget` is the token budget prompt composition targets (the mind's
/// `max_prompt_tokens`); it is passed in because `ene-ai` does not depend on
/// `ene-mind`. For every task that generates a completion — `chat`, plus
/// `proactive` when it is configured — the effective window
/// ([`AiConfig::effective_window_for_task`]) is compared against
/// `prompt_budget + tasks.<task>.max_tokens`. A window that falls short means
/// prompt sections are silently dropped every turn, so the caller should warn.
///
/// Only tasks whose window is known at startup are checked: a local model's
/// [`LocalModelDef::context_size`], or an explicit operator
/// [`AiProviderDef::context_window`] override. A cloud task with no override
/// learns its real window from the provider at runtime, so it is skipped here
/// rather than warned about on the conservative
/// [`crate::context_window::DEFAULT_CONTEXT_WINDOW`] floor.
#[must_use]
pub fn validate_context_budgets(ai: &AiConfig, prompt_budget: u32) -> Vec<ContextBudgetIssue> {
    let mut issues = Vec::new();
    check_task_budget(ai, "chat", &ai.tasks.chat, prompt_budget, &mut issues);
    if let Some(proactive) = ai.tasks.proactive.as_ref() {
        check_task_budget(ai, "proactive", proactive, prompt_budget, &mut issues);
    }
    issues
}

/// Compare one task's effective window against its required budget and record
/// a [`ContextBudgetIssue`] when the window is too small.
///
/// Only tasks whose window is known at startup are checked: a local model's
/// [`LocalModelDef::context_size`], or an explicit operator
/// [`AiProviderDef::context_window`] override. A cloud task with no override
/// learns its real window from the provider at runtime, so there is nothing to
/// validate yet and it is skipped (avoids false warnings for the default
/// cloud setup).
fn check_task_budget(
    ai: &AiConfig,
    task_name: &str,
    task: &TaskRef,
    prompt_budget: u32,
    issues: &mut Vec<ContextBudgetIssue>,
) {
    let advertised = local_advertised_window(ai, task);
    let user_configured = ai
        .providers
        .get(&task.provider)
        .and_then(|def| def.context_window);
    if advertised.is_none() && user_configured.is_none() {
        return;
    }
    let window = ai.effective_window_for_task(task, advertised);
    let response_reserve = task.max_tokens.unwrap_or(0);
    let required = prompt_budget.saturating_add(response_reserve);
    if window.effective < required {
        issues.push(ContextBudgetIssue {
            task: task_name.to_string(),
            model: task_model_label(task),
            effective_window: window.effective,
            prompt_budget,
            response_reserve,
            required,
        });
    }
}

/// The context window a local model advertises for a task, or `None` for a
/// cloud task (whose window is learned from the provider at runtime).
fn local_advertised_window(ai: &AiConfig, task: &TaskRef) -> Option<u32> {
    if !AiConfig::is_local_provider(&task.provider) {
        return None;
    }
    task.model
        .as_deref()
        .and_then(|name| ai.local_models.get(name))
        .map(|def| def.context_size)
}

/// Human-readable model label for a task: the local model key when the task
/// routes to the local provider, otherwise the cloud model name (falling back
/// to the provider name when the task names no model).
fn task_model_label(task: &TaskRef) -> String {
    if AiConfig::is_local_provider(&task.provider) {
        return task
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| LOCAL_PROVIDER.to_string());
    }
    task.model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| task.provider.clone())
}

/// Emit a `tracing::warn!` for each under-sized context window.
///
/// Convenience wrapper over [`validate_context_budgets`] for startup paths
/// that only need the side effect. `prompt_budget` is the mind's
/// `max_prompt_tokens`.
pub fn warn_on_context_budget_issues(ai: &AiConfig, prompt_budget: u32) {
    for issue in validate_context_budgets(ai, prompt_budget) {
        tracing::warn!(
            component = "AiConfig",
            task = %issue.task,
            model = %issue.model,
            effective_window = issue.effective_window,
            required = issue.required,
            prompt_budget = issue.prompt_budget,
            response_reserve = issue.response_reserve,
            "context window too small for the configured prompt budget; {}",
            issue.message()
        );
    }
    report_assumed_context_windows(ai);
}

/// Report every task running on the assumed default context window.
///
/// [`validate_context_budgets`] deliberately skips tasks whose window is
/// unknown, to avoid warning about the conservative floor on a normal cloud
/// setup. But the floor is not hypothetical: prompt packing budgets against
/// [`crate::context_window::DEFAULT_CONTEXT_WINDOW`] for those tasks, so a
/// 200k-window model that simply does not advertise its limit has its prompt
/// truncated to 8192 tokens with nothing said about it. Reported at info level
/// — this is a "you may want to set this" note, not a misconfiguration.
fn report_assumed_context_windows(ai: &AiConfig) {
    let report = |task_name: &str, task: &TaskRef| {
        if local_advertised_window(ai, task).is_some()
            || ai
                .providers
                .get(&task.provider)
                .and_then(|def| def.context_window)
                .is_some()
        {
            return;
        }
        tracing::info!(
            component = "AiConfig",
            task = task_name,
            model = %task_model_label(task),
            provider = %task.provider,
            assumed_window = crate::context_window::DEFAULT_CONTEXT_WINDOW,
            "no context window advertised by the provider or set in config; \
             assuming the conservative default. Set \
             `ai.providers.{}.context_window` to use the model's real limit.",
            task.provider
        );
    };

    report("chat", &ai.tasks.chat);
    if let Some(proactive) = ai.tasks.proactive.as_ref() {
        report("proactive", proactive);
    }
}

/// Lightweight API-key validation for an OpenAI-compatible provider.
///
/// Performs a `GET {base_url}/models` request with a short timeout so an
/// invalid key is reported before the first turn instead of surfacing as an
/// HTTP 401 mid-conversation. Sends no user data. An empty key fails fast
/// with [`crate::AiError::MissingApiKey`].
pub async fn validate_api_key(base_url: &str, api_key: &str) -> Result<(), crate::error::AiError> {
    if api_key.trim().is_empty() {
        return Err(crate::error::AiError::MissingApiKey(
            "resolved API key is empty; set the provider api_key or the configured env var"
                .to_string(),
        ));
    }

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| {
            crate::error::AiError::Llm(LlmProviderError::Provider(format!(
                "validation HTTP client init failed: {e}"
            )))
        })?;

    let response = client
        .get(url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| {
            crate::error::AiError::Llm(LlmProviderError::Network(format!(
                "API key validation request failed: {e}"
            )))
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let raw = response.text().await.unwrap_or_default();
    let snippet: String = raw.chars().take(200).collect();
    Err(crate::error::AiError::Llm(match status.as_u16() {
        401 | 403 => LlmProviderError::Auth(format!("API key rejected: {snippet}")),
        429 => LlmProviderError::RateLimit(snippet),
        _ => LlmProviderError::Provider(format!("API key validation HTTP {status}: {snippet}")),
    }))
}

impl AiConfig {
    /// Whether `name` is the reserved local provider.
    #[must_use]
    pub fn is_local_provider(name: &str) -> bool {
        name == LOCAL_PROVIDER
    }

    /// Look up a named cloud provider definition.
    pub fn get_provider(&self, name: &str) -> Result<&AiProviderDef, LlmProviderError> {
        self.providers
            .get(name)
            .ok_or_else(|| LlmProviderError::Provider(format!("unknown AI provider: {name:?}")))
    }

    /// Look up a named entry in [`AiConfig::local_models`].
    pub fn get_local_model(&self, name: &str) -> Result<&LocalModelDef, LlmProviderError> {
        self.local_models
            .get(name)
            .ok_or_else(|| LlmProviderError::Provider(format!("unknown local model: {name:?}")))
    }

    /// Resolve a local model for a task with `provider: "local"`.
    pub fn resolve_local_model_for_task(
        &self,
        task: &TaskRef,
    ) -> Result<ResolvedLocalModel, LlmProviderError> {
        if !Self::is_local_provider(&task.provider) {
            return Err(LlmProviderError::Provider(format!(
                "task provider {:?} is not {:?}",
                task.provider, LOCAL_PROVIDER
            )));
        }
        let name = task
            .model
            .as_deref()
            .filter(|m| !m.trim().is_empty())
            .ok_or_else(|| {
                LlmProviderError::Provider(format!(
                    "task with provider {LOCAL_PROVIDER:?} requires model (local_models key)"
                ))
            })?;
        let def = self.get_local_model(name)?;
        Ok(ResolvedLocalModel::from_named(name, def))
    }

    /// Resolve a [`TaskRef`] to its cloud provider and effective overrides.
    pub fn resolve_task_ref<'a>(
        &'a self,
        task: &'a TaskRef,
    ) -> Result<ResolvedTaskRef<'a>, LlmProviderError> {
        if Self::is_local_provider(&task.provider) {
            return Err(LlmProviderError::Provider(format!(
                "task provider {:?} is local; use resolve_local_model_for_task",
                task.provider
            )));
        }
        let provider = self.get_provider(&task.provider)?;
        let model = task.model.as_deref().filter(|m| !m.trim().is_empty());
        Ok(ResolvedTaskRef {
            provider,
            model,
            max_tokens: task.max_tokens,
            dimensions: task.dimensions,
        })
    }

    /// Compute the effective context-window budget for a task.
    ///
    /// `provider_advertised` is the window the backend reports for itself —
    /// [`ene_plugin_proto::LlmProviderSpec::context_window`] for a plugin
    /// provider, or `Some(LocalModelDef.context_size)` for a local model. It
    /// is combined with any `ai.providers.<name>.context_window` override on
    /// the task's provider (as `min`, so config can only shrink the model's
    /// stated limit) and the task's `max_tokens` response reserve, via
    /// [`crate::context_window::effective_window`]. The safety margin uses
    /// the heuristic default; callers with measured usage can recompute
    /// with a zero margin.
    #[must_use]
    pub fn effective_window_for_task(
        &self,
        task: &TaskRef,
        provider_advertised: Option<u32>,
    ) -> crate::context_window::EffectiveWindow {
        let user_configured = self
            .providers
            .get(&task.provider)
            .and_then(|def| def.context_window);
        crate::context_window::effective_window(
            provider_advertised,
            user_configured,
            task.max_tokens,
            crate::context_window::DEFAULT_SAFETY_MARGIN_FRACTION,
        )
    }

    /// The context window a task's model advertises from config alone.
    ///
    /// Returns the local model's [`LocalModelDef::context_size`] when the task
    /// routes to the local provider, or the operator's
    /// `ai.providers.<name>.context_window` override otherwise. A cloud task
    /// with no override returns `None` — its real window is only learned from
    /// the provider at runtime — so callers fall back to
    /// [`crate::context_window::DEFAULT_CONTEXT_WINDOW`].
    #[must_use]
    pub fn advertised_window_for_task(&self, task: &TaskRef) -> Option<u32> {
        if AiConfig::is_local_provider(&task.provider) {
            return task
                .model
                .as_deref()
                .and_then(|name| self.local_models.get(name))
                .map(|def| def.context_size);
        }
        self.providers
            .get(&task.provider)
            .and_then(|def| def.context_window)
    }

    /// Resolve chat settings for an optional task (`None` → [`AiConfig::tasks`] chat).
    pub fn resolve_chat_task(
        &self,
        task: Option<&TaskRef>,
    ) -> Result<ResolvedChat, LlmProviderError> {
        let task = task.unwrap_or(&self.tasks.chat);
        self.resolve_openai_chat_task(task)
    }

    /// Resolve main conversation chat settings.
    pub fn resolve_chat(&self) -> Result<ResolvedChat, LlmProviderError> {
        self.resolve_chat_task(None)
    }

    /// Resolve an ordered list of cloud chat candidates for failover.
    ///
    /// The configured chat provider is first (highest priority), followed by
    /// every other cloud provider in [`AiConfig::providers`] insertion order.
    /// Local providers are excluded (chat requires an OpenAI-compatible API).
    /// Candidates that fail to resolve (missing base URL / model) are skipped.
    #[must_use]
    pub fn resolve_chat_candidates(&self) -> Vec<ChatCandidate> {
        let mut candidates = Vec::new();
        let mut seen = std::collections::HashSet::new();

        if let Ok(resolved) = self.resolve_chat()
            && seen.insert(self.tasks.chat.provider.clone())
        {
            candidates.push(ChatCandidate {
                provider: self.tasks.chat.provider.clone(),
                kind: self
                    .providers
                    .get(&self.tasks.chat.provider)
                    .map_or_else(String::new, |def| {
                        crate::config::canonical_provider_kind(&def.kind).to_string()
                    }),
                base_url: resolved.base_url,
                api_key: resolved.api_key,
                model: resolved.model,
                max_tokens: resolved.max_tokens,
            });
        }

        // Only HTTP providers are health-probed here; plugin-provided kinds
        // are checked over IPC instead.
        for (name, def) in &self.providers {
            if seen.contains(name) || !def.is_openai_compatible() {
                continue;
            }
            seen.insert(name.clone());
            let Ok(resolved_url) = resolve_base_url(&def.base_url) else {
                continue;
            };
            // Use the chat task's model for fallback providers since they
            // share the same task routing; skip if no model is configured.
            let Some(model) = self
                .tasks
                .chat
                .model
                .as_deref()
                .filter(|m| !m.trim().is_empty())
            else {
                continue;
            };
            candidates.push(ChatCandidate {
                provider: name.clone(),
                kind: crate::config::canonical_provider_kind(&def.kind).to_string(),
                base_url: resolved_url,
                api_key: def.api_key.resolve_api_key(),
                model: model.to_string(),
                max_tokens: self.tasks.chat.max_tokens,
            });
        }

        candidates
    }

    /// Resolve classifier chat settings (falls back to chat task).
    pub fn resolve_classifier(&self) -> Result<ResolvedChat, LlmProviderError> {
        self.resolve_chat_task(self.tasks.classifier.as_ref())
    }

    /// Resolve proactive generation chat settings (falls back to chat task).
    ///
    /// Generation must use an OpenAI-compatible provider; `provider: "local"` is decision-only.
    pub fn resolve_proactive_generation(&self) -> Result<ResolvedChat, LlmProviderError> {
        if let Some(proactive) = self.tasks.proactive.as_ref() {
            if Self::is_local_provider(&proactive.provider) {
                return self.resolve_chat();
            }
            if self
                .get_provider(&proactive.provider)?
                .is_openai_compatible()
            {
                return self.resolve_openai_chat_task(proactive);
            }
        }
        self.resolve_chat()
    }

    /// True when proactive/chat generation may receive an image part.
    ///
    /// Uses `tasks.proactive.supports_vision` when that task is a cloud generation
    /// override; otherwise `tasks.chat.supports_vision`.
    #[must_use]
    pub fn proactive_generation_supports_vision(&self) -> bool {
        if let Some(proactive) = self.tasks.proactive.as_ref()
            && !Self::is_local_provider(&proactive.provider)
        {
            return proactive.supports_vision;
        }
        self.tasks.chat.supports_vision
    }

    /// Resolve embedding backend settings for [`AiConfig::tasks`] embedding task.
    pub fn resolve_embedding(&self) -> Result<ResolvedEmbedding, LlmProviderError> {
        if Self::is_local_provider(&self.tasks.embedding.provider) {
            let local = self.resolve_local_model_for_task(&self.tasks.embedding)?;
            return Ok(ResolvedEmbedding::Local(local));
        }
        let resolved = self.resolve_task_ref(&self.tasks.embedding)?;
        let def = resolved.provider;
        if !def.is_openai_compatible() {
            return Err(LlmProviderError::Provider(format!(
                "embedding provider {:?} has kind {:?}; only openai-family kinds ({:?} / {:?}) are supported here",
                self.tasks.embedding.provider,
                def.kind,
                crate::config::OPENAI_PROVIDER_KIND,
                crate::config::LEGACY_OPENAI_COMPATIBLE_KIND
            )));
        }
        let model = resolved.model.ok_or_else(|| {
            LlmProviderError::Provider(
                "embedding task requires model for an openai-family provider".to_string(),
            )
        })?;
        let dimensions = resolved.dimensions.unwrap_or(1536);
        // `resolve_base_url` only errors when both `base_url` and
        // `OPENAI_BASE_URL` are unset; the OpenAI plugin then uses its own
        // default, so resolution must not hard-fail here (chat would keep
        // working while embedding setup would not).
        let base_url = resolve_base_url(&def.base_url)
            .unwrap_or_else(|_| crate::config::DEFAULT_OPENAI_API_BASE.to_string());
        Ok(ResolvedEmbedding::Cloud {
            kind: crate::config::canonical_provider_kind(&def.kind).to_string(),
            base_url,
            api_key: def.api_key.resolve_api_key(),
            model: model.to_string(),
            dimensions,
            query_prefix: self
                .tasks
                .embedding
                .query_prefix
                .clone()
                .filter(|p| !p.trim().is_empty()),
        })
    }

    fn resolve_openai_chat_task(&self, task: &TaskRef) -> Result<ResolvedChat, LlmProviderError> {
        if Self::is_local_provider(&task.provider) {
            return Err(LlmProviderError::Provider(
                "chat tasks cannot use the local provider; configure a cloud provider kind"
                    .to_string(),
            ));
        }
        let resolved = self.resolve_task_ref(task)?;
        let def = resolved.provider;
        if !def.is_openai_compatible() {
            return Err(LlmProviderError::Provider(format!(
                "chat provider {:?} has kind {:?}; only openai-family kinds ({:?} / {:?}) are supported here",
                task.provider,
                def.kind,
                crate::config::OPENAI_PROVIDER_KIND,
                crate::config::LEGACY_OPENAI_COMPATIBLE_KIND
            )));
        }
        let model = resolved.model.ok_or_else(|| {
            LlmProviderError::Provider(format!(
                "task for provider {:?} requires model",
                task.provider
            ))
        })?;
        Ok(ResolvedChat {
            base_url: resolve_base_url(&def.base_url)?,
            api_key: def.api_key.resolve_api_key(),
            model: model.to_string(),
            max_tokens: resolved.max_tokens,
        })
    }

    /// Resolve TTS provider settings from [`AiConfig::tts`].
    ///
    /// Returns `None` when the provider is `"none"` (TTS disabled). The `speed`
    /// multiplier is clamped to `[0.1, 5.0]` with a warning when adjusted.
    #[must_use]
    pub fn resolve_tts(&self) -> Option<ResolvedTts> {
        if self.tts.provider == "none" {
            return None;
        }
        let speed = clamp_with_warn(self.tts.speed, 0.1, 5.0, "ai.tts.speed");
        Some(ResolvedTts {
            provider: self.tts.provider.clone(),
            model: self.tts.model.clone(),
            voice: (!self.tts.voice.trim().is_empty()).then(|| self.tts.voice.clone()),
            speed,
            language: (!self.tts.language.trim().is_empty()).then(|| self.tts.language.clone()),
        })
    }

    /// Resolve STT provider settings from [`AiConfig::stt`].
    ///
    /// Returns `None` when the provider is `"none"` (STT disabled).
    #[must_use]
    pub fn resolve_stt(&self) -> Option<ResolvedStt> {
        if self.stt.provider == "none" {
            return None;
        }
        Some(ResolvedStt {
            provider: self.stt.provider.clone(),
            model: self.stt.model.clone(),
            language: (!self.stt.language.trim().is_empty()).then(|| self.stt.language.clone()),
        })
    }

    /// Resolve VAD engine settings from [`AiConfig::vad`].
    ///
    /// Returns `None` when the provider is `"none"` (VAD disabled). Engine
    /// tuning (threshold, model paths) lives in the provider plugin's own
    /// config (`plugins.list.<name>.config`), not in `AiConfig`.
    #[must_use]
    pub fn resolve_vad(&self) -> Option<ResolvedVad> {
        if self.vad.provider == "none" {
            return None;
        }
        Some(ResolvedVad {
            provider: self.vad.provider.clone(),
        })
    }
}

/// Clamp `value` to `[min, max]`, emitting a `tracing::warn!` when it is
/// adjusted. Used to sanitize numeric config values at the resolve boundary.
fn clamp_with_warn(value: f32, min: f32, max: f32, field: &str) -> f32 {
    let clamped = value.clamp(min, max);
    if (clamped - value).abs() > f32::EPSILON {
        tracing::warn!(
            component = "AiConfig",
            field,
            value,
            clamped,
            "config value out of range; clamping"
        );
    }
    clamped
}

// ── Provider health monitoring and failover ─────────────────────────────
//
// Upstream reachability is probed *through* the provider plugins (a minimal
// chat ping), so the plugin exercises its own endpoint and reports the
// outcome over IPC; the host classifies, caches, and routes on the results.
// Process supervision (child liveness, restarts, circuit breaker) is a
// separate concern owned by `ene-plugin-host`.

/// Outcome of a single provider health probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderHealthStatus {
    /// Provider responded successfully within the timeout.
    Healthy,
    /// Provider responded but with elevated latency (> 2× the median).
    Degraded {
        /// Measured round-trip latency.
        latency_ms: u64,
    },
    /// Provider rejected the API key (HTTP 401/403).
    AuthFailed,
    /// Provider is rate-limiting (HTTP 429).
    RateLimited,
    /// Provider is unreachable (network error, DNS, TLS, timeout).
    Unreachable,
    /// Provider returned an unexpected HTTP status.
    ServerError {
        /// HTTP status code; `0` when the probe could not observe one
        /// (chat-ping probes classify from typed provider errors instead).
        status: u16,
    },
    /// Health has not been checked yet.
    Unknown,
}

impl ProviderHealthStatus {
    /// Whether the provider is usable for a chat request right now.
    ///
    /// `Unknown` returns `true` so unprobed providers can be used during
    /// bootstrap without requiring an upfront health check.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded { .. } | Self::Unknown)
    }

    /// Stable English status code for the diagnostics contract.
    #[must_use]
    pub const fn status_code(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded { .. } => "degraded",
            Self::AuthFailed => "auth_failed",
            Self::RateLimited => "rate_limited",
            Self::Unreachable => "unreachable",
            Self::ServerError { .. } => "server_error",
            Self::Unknown => "unknown",
        }
    }
}

/// A single health probe result for one provider.
#[derive(Debug, Clone)]
pub struct ProviderHealthReport {
    /// Provider name (key in `ai.providers`).
    pub provider: String,
    /// Probe outcome.
    pub status: ProviderHealthStatus,
    /// Measured round-trip latency (0 if unreachable).
    pub latency_ms: u64,
    /// Human-readable error detail, if any.
    pub error: Option<String>,
    /// When the probe was performed.
    pub checked_at: Instant,
}

/// Probe a provider with a minimal chat ping.
///
/// Sends **no** user data — only the literal `"ping"` message via
/// [`LlmProvider::chat_completion`], mirroring the local-model warm-up
/// pattern. The plugin exercises its real endpoint and maps transport
/// outcomes onto typed [`LlmProviderError`] variants; this function
/// classifies them into [`ProviderHealthStatus`].
pub async fn probe_provider_health(
    provider: &dyn LlmProvider,
    provider_name: &str,
    timeout: Duration,
) -> ProviderHealthReport {
    let start = Instant::now();
    let messages = [LlmMessage::User {
        parts: vec![UserMessagePart::Text {
            text: "ping".to_string(),
        }],
    }];
    let result = tokio::time::timeout(timeout, provider.chat_completion(&messages, None)).await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(_)) => ProviderHealthReport {
            provider: provider_name.to_string(),
            status: ProviderHealthStatus::Healthy,
            latency_ms,
            error: None,
            checked_at: start,
        },
        Ok(Err(e)) => {
            let status = match &e {
                LlmProviderError::Auth(_) => ProviderHealthStatus::AuthFailed,
                LlmProviderError::RateLimit(_) => ProviderHealthStatus::RateLimited,
                LlmProviderError::Network(_) | LlmProviderError::Timeout => {
                    ProviderHealthStatus::Unreachable
                }
                _ => ProviderHealthStatus::ServerError { status: 0 },
            };
            ProviderHealthReport {
                provider: provider_name.to_string(),
                status,
                latency_ms,
                error: Some(e.to_string()),
                checked_at: start,
            }
        }
        Err(_) => ProviderHealthReport {
            provider: provider_name.to_string(),
            status: ProviderHealthStatus::Unreachable,
            latency_ms,
            error: Some("health probe timed out".to_string()),
            checked_at: start,
        },
    }
}

/// A recorded fallback event.
#[derive(Debug, Clone)]
pub struct FallbackRecord {
    /// Provider that failed.
    pub from: String,
    /// Provider that was selected instead.
    pub to: String,
    /// Reason for the fallback.
    pub reason: String,
    /// When the fallback occurred.
    pub at: Instant,
}

/// In-memory health monitor with TTL caching and fallback history.
///
/// Thread-safe via `Arc<Mutex<..>>`. The runtime actor holds one instance
/// and shares it with the diagnostics facade for `/doctor` queries.
#[derive(Clone)]
pub struct ProviderHealthMonitor {
    inner: Arc<Mutex<MonitorInner>>,
}

struct MonitorInner {
    /// Cached health reports keyed by provider name.
    reports: HashMap<String, ProviderHealthReport>,
    /// How long a cached report is considered fresh.
    ttl: Duration,
    /// Recent fallback events (bounded ring buffer).
    fallback_history: VecDeque<FallbackRecord>,
    /// Maximum fallback records to retain.
    max_history: usize,
}

impl std::fmt::Debug for ProviderHealthMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("ProviderHealthMonitor")
            .field("cached_providers", &inner.reports.len())
            .field("fallback_count", &inner.fallback_history.len())
            .finish()
    }
}

impl ProviderHealthMonitor {
    /// Create a new monitor with the given cache TTL and history capacity.
    #[must_use]
    pub fn new(ttl: Duration, max_history: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MonitorInner {
                reports: HashMap::new(),
                ttl,
                fallback_history: VecDeque::new(),
                max_history,
            })),
        }
    }

    /// Record a health probe result, replacing any previous entry.
    pub fn record(&self, report: ProviderHealthReport) {
        let mut inner = self.inner.lock();
        inner.reports.insert(report.provider.clone(), report);
    }

    /// Get the cached health status for a provider, if fresh.
    ///
    /// Returns `None` when no report exists or the cached report has expired.
    pub fn get_fresh(&self, provider: &str) -> Option<ProviderHealthReport> {
        let inner = self.inner.lock();
        inner.reports.get(provider).and_then(|r| {
            if r.checked_at.elapsed() < inner.ttl {
                Some(r.clone())
            } else {
                None
            }
        })
    }

    /// Get the cached health status regardless of freshness.
    pub fn get_any(&self, provider: &str) -> Option<ProviderHealthReport> {
        let inner = self.inner.lock();
        inner.reports.get(provider).cloned()
    }

    /// Record a fallback event.
    pub fn record_fallback(&self, from: &str, to: &str, reason: &str) {
        let mut inner = self.inner.lock();
        if inner.fallback_history.len() >= inner.max_history {
            inner.fallback_history.pop_front();
        }
        inner.fallback_history.push_back(FallbackRecord {
            from: from.to_string(),
            to: to.to_string(),
            reason: reason.to_string(),
            at: Instant::now(),
        });
    }

    /// Snapshot of recent fallback events (newest last).
    pub fn fallback_history(&self) -> Vec<FallbackRecord> {
        let inner = self.inner.lock();
        inner.fallback_history.iter().cloned().collect()
    }

    /// Snapshot of all cached health reports.
    pub fn all_reports(&self) -> Vec<ProviderHealthReport> {
        let inner = self.inner.lock();
        inner.reports.values().cloned().collect()
    }
}

impl Default for ProviderHealthMonitor {
    fn default() -> Self {
        Self::new(Duration::from_mins(1), 32)
    }
}

/// Result of a failover selection.
#[derive(Debug, Clone)]
pub struct FailoverSelection {
    /// The selected candidate.
    pub candidate: ChatCandidate,
    /// Providers that were tried and skipped before the selection, with reasons.
    pub skipped: Vec<(String, String)>,
    /// Whether a fallback occurred (selected is not the first candidate).
    pub fell_back: bool,
}

/// Select the first healthy chat candidate in priority order.
///
/// Builds each candidate's provider through the host registry and probes it
/// with a minimal chat ping (using the monitor's TTL cache to avoid
/// redundant probes), returning the first one whose health status is
/// [`ProviderHealthStatus::is_available`]. If a fallback occurs, the event is
/// recorded in the monitor. Sends **no** user data during probes.
///
/// Returns `None` only when there are no candidates at all.
pub async fn select_healthy_chat(
    candidates: &[ChatCandidate],
    monitor: &ProviderHealthMonitor,
    host: &dyn ProviderHost,
    config: &ene_config::EneConfig,
    timeout: Duration,
) -> Option<FailoverSelection> {
    if candidates.is_empty() {
        return None;
    }

    let mut skipped = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        let report = if let Some(cached) = monitor.get_fresh(&candidate.provider) {
            cached
        } else {
            let fresh = probe_candidate(candidate, host, config, timeout).await;
            monitor.record(fresh.clone());
            fresh
        };

        if report.status.is_available() {
            let fell_back = index > 0;
            if fell_back && let Some(primary) = candidates.first() {
                let reason = skipped
                    .iter()
                    .map(|(p, r)| format!("{p}: {r}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                monitor.record_fallback(&primary.provider, &candidate.provider, &reason);
            }
            return Some(FailoverSelection {
                candidate: candidate.clone(),
                skipped,
                fell_back,
            });
        }

        let reason = report
            .error
            .clone()
            .unwrap_or_else(|| format!("{:?}", report.status));
        skipped.push((candidate.provider.clone(), reason));
    }

    // No candidate was healthy. Fall back to the first candidate anyway so
    // the turn can attempt to proceed (the provider may recover mid-request),
    // but record that every probe failed.
    let primary = candidates.first()?;
    let reason = skipped
        .iter()
        .map(|(p, r)| format!("{p}: {r}"))
        .collect::<Vec<_>>()
        .join("; ");
    tracing::warn!(
        component = "health",
        reason = %reason,
        "all providers unhealthy; using primary candidate anyway"
    );
    Some(FailoverSelection {
        candidate: primary.clone(),
        skipped,
        fell_back: false,
    })
}

/// Build and probe one chat candidate through the host registry.
///
/// A candidate whose provider cannot be created (plugin absent, host down)
/// is reported unavailable rather than skipped silently, so the monitor
/// carries a report for every candidate and the doctor can show it.
async fn probe_candidate(
    candidate: &ChatCandidate,
    host: &dyn ProviderHost,
    config: &ene_config::EneConfig,
    timeout: Duration,
) -> ProviderHealthReport {
    let task = TaskRef {
        provider: candidate.provider.clone(),
        model: Some(candidate.model.clone()),
        max_tokens: candidate.max_tokens,
        ..TaskRef::default()
    };
    match host
        .create_llm_provider(&candidate.kind, config, &task)
        .await
    {
        Ok(provider) => {
            probe_provider_health(provider.as_ref(), &candidate.provider, timeout).await
        }
        Err(e) => ProviderHealthReport {
            provider: candidate.provider.clone(),
            status: ProviderHealthStatus::Unreachable,
            latency_ms: 0,
            error: Some(format!("provider creation failed: {e}")),
            checked_at: Instant::now(),
        },
    }
}

/// Probe every chat candidate through the host registry.
///
/// Unlike [`select_healthy_chat`], this never consults or updates a health
/// monitor: every candidate is probed fresh, so on-demand diagnostics
/// (CLI `/doctor`) reflect the current state without warming the turn-path
/// cache, and backups are probed even when the primary is healthy.
pub async fn probe_chat_candidates(
    candidates: &[ChatCandidate],
    host: &dyn ProviderHost,
    config: &ene_config::EneConfig,
    timeout: Duration,
) -> Vec<ProviderHealthReport> {
    let mut reports = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        reports.push(probe_candidate(candidate, host, config, timeout).await);
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::LlmResponseChunk;
    use crate::traits::EmbeddingProvider;
    use crate::{AudioProviderError, EmbeddingError, TtsProvider};
    use async_trait::async_trait;
    use std::pin::Pin;
    use tokio_stream::Stream;

    #[test]
    fn resolve_tts_disabled_by_default() {
        let cfg = AiConfig::default();
        assert!(cfg.resolve_tts().is_none());
    }

    #[test]
    fn resolve_stt_disabled_by_default() {
        let cfg = AiConfig::default();
        assert!(cfg.resolve_stt().is_none());
    }

    #[test]
    fn resolve_vad_disabled_by_default() {
        let cfg = AiConfig::default();
        assert!(cfg.resolve_vad().is_none());
    }

    #[test]
    fn resolve_tts_maps_fields_and_clamps_speed() {
        let mut cfg = AiConfig::default();
        cfg.tts.provider = "kokoro".to_string();
        cfg.tts.model = "kokoro-v1.0".to_string();
        cfg.tts.voice = "af_heart".to_string();
        cfg.tts.language = "en".to_string();
        cfg.tts.speed = 99.0;

        let resolved = cfg.resolve_tts().expect("tts enabled");
        assert_eq!(resolved.provider, "kokoro");
        assert_eq!(resolved.model, "kokoro-v1.0");
        assert_eq!(resolved.voice.as_deref(), Some("af_heart"));
        assert_eq!(resolved.language.as_deref(), Some("en"));
        assert!((resolved.speed - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_tts_empty_voice_and_language_are_none() {
        let mut cfg = AiConfig::default();
        cfg.tts.provider = "kokoro".to_string();
        cfg.tts.voice = "   ".to_string();
        cfg.tts.language = String::new();

        let resolved = cfg.resolve_tts().expect("tts enabled");
        assert!(resolved.voice.is_none());
        assert!(resolved.language.is_none());
    }

    #[test]
    fn resolve_stt_maps_fields() {
        let mut cfg = AiConfig::default();
        cfg.stt.provider = "whisper".to_string();
        cfg.stt.model = "whisper.gguf".to_string();
        cfg.stt.language = "ja".to_string();

        let resolved = cfg.resolve_stt().expect("stt enabled");
        assert_eq!(resolved.provider, "whisper");
        assert_eq!(resolved.model, "whisper.gguf");
        assert_eq!(resolved.language.as_deref(), Some("ja"));
    }

    #[test]
    fn resolve_vad_maps_provider_only() {
        let mut cfg = AiConfig::default();
        cfg.vad.provider = "silero".to_string();

        let resolved = cfg.resolve_vad().expect("vad enabled");
        assert_eq!(resolved.provider, "silero");
    }

    #[test]
    fn clamp_with_warn_noop_in_range() {
        assert!((clamp_with_warn(0.5, 0.0, 1.0, "x") - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn validate_provider_kinds_accepts_builtin_and_plugin() {
        let mut cfg = AiConfig::default();
        cfg.providers.insert(
            "claude".to_string(),
            AiProviderDef {
                kind: "anthropic".to_string(),
                ..AiProviderDef::default()
            },
        );
        cfg.providers.insert(
            "custom".to_string(),
            AiProviderDef {
                kind: "my-plugin-provider".to_string(),
                ..AiProviderDef::default()
            },
        );
        assert!(validate_provider_kinds(&cfg).is_empty());
    }

    #[test]
    fn validate_provider_kinds_flags_typo() {
        let mut cfg = AiConfig::default();
        cfg.providers.insert(
            "bad".to_string(),
            AiProviderDef {
                kind: "openai-compatible".to_string(),
                ..AiProviderDef::default()
            },
        );
        let issues = validate_provider_kinds(&cfg);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues.first(),
            Some(&SettingsIssue::SuspiciousKind {
                provider: "bad".to_string(),
                kind: "openai-compatible".to_string(),
                suggestion: Some("openai_compatible".to_string()),
            })
        );
    }

    #[test]
    fn validate_settings_reports_suspicious_kind() {
        let mut ai = AiConfig::default();
        ai.providers.insert(
            "typo".to_string(),
            AiProviderDef {
                kind: "anthroic".to_string(),
                ..AiProviderDef::default()
            },
        );
        let mut config = ene_config::EneConfig::default();
        config.set_section(&ai).expect("ai config merges");
        let issues = validate_settings(&config);
        assert!(issues.iter().any(|i| matches!(
            i,
            SettingsIssue::SuspiciousKind { kind, .. } if kind == "anthroic"
        )));
    }

    #[test]
    fn effective_window_for_task_honors_advertised_and_reserve() {
        let mut cfg = AiConfig::default();
        cfg.providers.insert(
            "default".to_string(),
            AiProviderDef {
                kind: "anthropic".to_string(),
                ..AiProviderDef::default()
            },
        );
        let task = TaskRef {
            provider: "default".to_string(),
            max_tokens: Some(4_096),
            ..TaskRef::default()
        };
        let w = cfg.effective_window_for_task(&task, Some(200_000));
        assert_eq!(w.effective, 200_000);
        assert_eq!(w.response_reserve, 4_096);
        assert_eq!(
            w.safety_margin,
            200_000 / crate::context_window::DEFAULT_SAFETY_MARGIN_FRACTION
        );
    }

    #[test]
    fn effective_window_for_task_applies_config_override() {
        let mut cfg = AiConfig::default();
        cfg.providers.insert(
            "default".to_string(),
            AiProviderDef {
                kind: "anthropic".to_string(),
                context_window: Some(32_000),
                ..AiProviderDef::default()
            },
        );
        let task = TaskRef {
            provider: "default".to_string(),
            ..TaskRef::default()
        };
        let w = cfg.effective_window_for_task(&task, Some(200_000));
        assert_eq!(w.effective, 32_000);
    }

    #[test]
    fn effective_window_for_task_defaults_when_unadvertised() {
        let cfg = AiConfig::default();
        let task = TaskRef {
            provider: "default".to_string(),
            max_tokens: None,
            ..TaskRef::default()
        };
        let w = cfg.effective_window_for_task(&task, None);
        assert_eq!(w.effective, crate::context_window::DEFAULT_CONTEXT_WINDOW);
    }

    #[test]
    fn advertised_window_reads_local_model_context_size() {
        let cfg = local_chat_config(16_384, 2_048);
        assert_eq!(
            cfg.advertised_window_for_task(&cfg.tasks.chat),
            Some(16_384)
        );
    }

    #[test]
    fn advertised_window_reads_provider_override_for_cloud() {
        let mut cfg = AiConfig::default();
        cfg.providers.insert(
            "default".to_string(),
            AiProviderDef {
                kind: "anthropic".to_string(),
                context_window: Some(128_000),
                ..AiProviderDef::default()
            },
        );
        let task = TaskRef {
            provider: "default".to_string(),
            ..TaskRef::default()
        };
        assert_eq!(cfg.advertised_window_for_task(&task), Some(128_000));
    }

    #[test]
    fn advertised_window_is_none_for_unconfigured_cloud_task() {
        let cfg = AiConfig::default();
        let task = TaskRef {
            provider: "default".to_string(),
            ..TaskRef::default()
        };
        assert_eq!(cfg.advertised_window_for_task(&task), None);
    }

    /// Build an [`AiConfig`] whose chat and proactive tasks both route to a
    /// local model with the given context size and chat output reserve. The
    /// proactive reserve is fixed at 2,048 (a typical local companion
    /// utterance).
    fn local_chat_config(context_size: u32, chat_max_tokens: u32) -> AiConfig {
        let mut cfg = AiConfig::default();
        cfg.local_models.insert(
            "gemma".to_string(),
            LocalModelDef {
                context_size,
                ..LocalModelDef::default()
            },
        );
        cfg.tasks.chat = TaskRef {
            provider: crate::config::LOCAL_PROVIDER.to_string(),
            model: Some("gemma".to_string()),
            max_tokens: Some(chat_max_tokens),
            ..TaskRef::default()
        };
        cfg.tasks.proactive = Some(TaskRef {
            provider: crate::config::LOCAL_PROVIDER.to_string(),
            model: Some("gemma".to_string()),
            max_tokens: Some(2_048),
            ..TaskRef::default()
        });
        cfg
    }

    #[test]
    fn validate_context_budgets_flags_undersized_local_window() {
        // The 2,048-token window cannot hold a 12,000 prompt even with a
        // modest local reserve.
        let cfg = local_chat_config(2_048, 2_048);
        let issues = validate_context_budgets(&cfg, 12_000);
        // Both the chat and proactive tasks share the undersized model.
        assert_eq!(issues.len(), 2);
        let chat = issues
            .iter()
            .find(|i| i.task == "chat")
            .expect("chat issue");
        assert_eq!(chat.model, "gemma");
        assert_eq!(chat.effective_window, 2_048);
        assert_eq!(chat.prompt_budget, 12_000);
        assert_eq!(chat.response_reserve, 2_048);
        assert_eq!(chat.required, 14_048);
        let msg = chat.message();
        assert!(msg.contains("2048"), "message: {msg}");
        assert!(msg.contains("14048"), "message: {msg}");
        assert!(msg.contains("12000"), "message: {msg}");
    }

    #[test]
    fn validate_context_budgets_passes_with_default_local_window() {
        let cfg = local_chat_config(16_384, 2_048);
        assert!(validate_context_budgets(&cfg, 12_000).is_empty());
    }

    #[test]
    fn assumed_window_is_reported_only_when_no_source_names_one() {
        // An unadvertised, unconfigured task silently runs on the conservative
        // default, so it is worth an info note even though it is not an issue.
        let mut cfg = AiConfig::default();
        cfg.tasks.chat = TaskRef {
            provider: "default".to_string(),
            ..TaskRef::default()
        };
        assert!(
            local_advertised_window(&cfg, &cfg.tasks.chat).is_none()
                && cfg
                    .providers
                    .get(&cfg.tasks.chat.provider)
                    .and_then(|d| d.context_window)
                    .is_none(),
            "fixture must have no window from either source"
        );

        // Naming the window in config removes the reason to report.
        if let Some(def) = cfg.providers.get_mut("default") {
            def.context_window = Some(200_000);
        }
        assert_eq!(
            cfg.advertised_window_for_task(&cfg.tasks.chat),
            Some(200_000),
            "an explicit override must be picked up as the advertised window"
        );
        assert!(validate_context_budgets(&cfg, 12_000).is_empty());
    }

    #[test]
    fn validate_context_budgets_warns_when_cloud_reserve_kept_on_local() {
        // 16,384 is enough for the prompt but NOT for a cloud-sized 8,192
        // output reserve (12,000 + 8,192 = 20,192). Keeping the cloud default
        // on a local chat model is exactly the misconfiguration the startup
        // warning targets, so chat is flagged while the modest-reserve
        // proactive task is not.
        let cfg = local_chat_config(16_384, 8_192);
        let issues = validate_context_budgets(&cfg, 12_000);
        assert_eq!(issues.len(), 1);
        let issue = issues.first().expect("one issue");
        assert_eq!(issue.task, "chat");
        assert_eq!(issue.effective_window, 16_384);
        assert_eq!(issue.required, 20_192);
    }

    #[test]
    fn validate_context_budgets_skips_cloud_without_override() {
        let cfg = AiConfig::default();
        assert!(validate_context_budgets(&cfg, 12_000).is_empty());
    }

    #[test]
    fn validate_context_budgets_flags_small_cloud_override() {
        let mut cfg = AiConfig::default();
        if let Some(def) = cfg.providers.get_mut("default") {
            def.context_window = Some(4_096);
        }
        cfg.tasks.chat.max_tokens = Some(8_192);
        let issues = validate_context_budgets(&cfg, 12_000);
        assert_eq!(issues.len(), 1);
        let issue = issues.first().expect("one issue");
        assert_eq!(issue.task, "chat");
        assert_eq!(issue.effective_window, 4_096);
        assert_eq!(issue.required, 20_192);
    }

    #[test]
    fn validate_context_budgets_ignores_classifier_and_embedding() {
        let mut cfg = local_chat_config(16_384, 2_048);
        cfg.tasks.embedding = TaskRef {
            provider: crate::config::LOCAL_PROVIDER.to_string(),
            model: Some("gemma".to_string()),
            max_tokens: None,
            ..TaskRef::default()
        };
        cfg.local_models
            .get_mut("gemma")
            .expect("gemma present")
            .context_size = 512;
        // The embedding task is not generative, but chat/proactive now share
        // the tiny window, so only those two are flagged.
        let issues = validate_context_budgets(&cfg, 12_000);
        assert!(issues.iter().all(|i| i.task != "embedding"));
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn health_status_availability() {
        assert!(ProviderHealthStatus::Healthy.is_available());
        assert!(ProviderHealthStatus::Degraded { latency_ms: 500 }.is_available());
        assert!(ProviderHealthStatus::Unknown.is_available());
        assert!(!ProviderHealthStatus::AuthFailed.is_available());
        assert!(!ProviderHealthStatus::RateLimited.is_available());
        assert!(!ProviderHealthStatus::Unreachable.is_available());
        assert!(!ProviderHealthStatus::ServerError { status: 500 }.is_available());
    }

    #[test]
    fn monitor_records_and_retrieves() {
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        let report = ProviderHealthReport {
            provider: "default".to_string(),
            status: ProviderHealthStatus::Healthy,
            latency_ms: 42,
            error: None,
            checked_at: Instant::now(),
        };
        monitor.record(report.clone());
        let fresh = monitor.get_fresh("default");
        assert!(fresh.is_some());
        assert_eq!(fresh.unwrap().latency_ms, 42);
        assert!(monitor.get_fresh("nonexistent").is_none());
    }

    #[test]
    fn monitor_fallback_history_bounded() {
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 3);
        for i in 0..5 {
            monitor.record_fallback(&format!("p{i}"), "fallback", "test");
        }
        let history = monitor.fallback_history();
        assert_eq!(history.len(), 3);
        assert_eq!(history.first().map(|r| r.from.as_str()), Some("p2"));
        assert_eq!(history.get(2).map(|r| r.from.as_str()), Some("p4"));
    }

    #[test]
    fn monitor_expired_report_not_fresh() {
        let monitor = ProviderHealthMonitor::new(Duration::from_millis(0), 8);
        let checked_at = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let report = ProviderHealthReport {
            provider: "default".to_string(),
            status: ProviderHealthStatus::Healthy,
            latency_ms: 10,
            error: None,
            checked_at,
        };
        monitor.record(report);
        assert!(monitor.get_fresh("default").is_none());
        assert!(monitor.get_any("default").is_some());
    }

    /// Which typed error the failing probe stub should produce.
    #[derive(Clone, Copy)]
    enum StubFailure {
        Auth,
        RateLimit,
        Network,
        Timeout,
        Server,
    }

    /// Stub provider whose `chat_completion` fails with a fixed error.
    struct FailingProvider {
        failure: StubFailure,
    }

    impl FailingProvider {
        fn error(&self) -> LlmProviderError {
            match self.failure {
                StubFailure::Auth => LlmProviderError::Auth("bad key".to_string()),
                StubFailure::RateLimit => LlmProviderError::RateLimit("slow down".to_string()),
                StubFailure::Network => LlmProviderError::Network("refused".to_string()),
                StubFailure::Timeout => LlmProviderError::Timeout,
                StubFailure::Server => LlmProviderError::Provider("HTTP 500: boom".to_string()),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for FailingProvider {
        fn name(&self) -> &'static str {
            "probe-stub"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
            LlmProviderError,
        > {
            Err(LlmProviderError::Provider(
                "probe stub does not stream".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<crate::LlmCompletion, LlmProviderError> {
            Err(self.error())
        }
    }

    struct HealthyProvider;

    #[async_trait]
    impl LlmProvider for HealthyProvider {
        fn name(&self) -> &'static str {
            "probe-healthy"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
            LlmProviderError,
        > {
            Err(LlmProviderError::Provider(
                "probe stub does not stream".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<crate::LlmCompletion, LlmProviderError> {
            Ok(crate::LlmCompletion::text_only("pong".to_string()))
        }
    }

    /// Boxes an `Arc<dyn LlmProvider>` so the stub host can hand out clones
    /// of the same provider per kind.
    struct ArcLlmProvider(Arc<dyn LlmProvider>);

    #[async_trait]
    impl LlmProvider for ArcLlmProvider {
        fn name(&self) -> &str {
            self.0.name()
        }

        async fn create_chat_stream(
            &self,
            messages: &[LlmMessage],
            tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
            LlmProviderError,
        > {
            self.0.create_chat_stream(messages, tools).await
        }

        async fn chat_completion(
            &self,
            messages: &[LlmMessage],
            json_schema: Option<serde_json::Value>,
        ) -> Result<crate::LlmCompletion, LlmProviderError> {
            self.0.chat_completion(messages, json_schema).await
        }
    }

    /// Stub host serving a fixed set of providers by kind.
    struct StubHost {
        llm: HashMap<String, Arc<dyn LlmProvider>>,
    }

    #[async_trait]
    impl ProviderHost for StubHost {
        async fn create_llm_provider(
            &self,
            kind: &str,
            _config: &ene_config::EneConfig,
            _task: &TaskRef,
        ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
            self.llm.get(kind).map_or_else(
                || {
                    Err(LlmProviderError::Provider(format!(
                        "No LlmProviderFactory registered for provider kind: '{kind}'"
                    )))
                },
                |provider| {
                    let provider: Box<dyn LlmProvider> =
                        Box::new(ArcLlmProvider(Arc::clone(provider)));
                    Ok(provider)
                },
            )
        }

        async fn create_embedding_provider(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Arc<dyn EmbeddingProvider>, EmbeddingError> {
            Err(EmbeddingError::Init(
                "stub host serves no embedding providers".to_string(),
            ))
        }

        async fn create_tts_provider(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
            Err(AudioProviderError::Provider(
                "stub host serves no TTS providers".to_string(),
            ))
        }
    }

    fn candidate(provider: &str, kind: &str) -> ChatCandidate {
        ChatCandidate {
            provider: provider.to_string(),
            kind: kind.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: "gpt-test".to_string(),
            max_tokens: None,
        }
    }

    #[tokio::test]
    async fn probe_classifies_typed_provider_errors() {
        let timeout = Duration::from_secs(1);
        let healthy = probe_provider_health(&HealthyProvider, "healthy", timeout).await;
        assert_eq!(healthy.status, ProviderHealthStatus::Healthy);

        for (failure, expected) in [
            (StubFailure::Auth, ProviderHealthStatus::AuthFailed),
            (StubFailure::RateLimit, ProviderHealthStatus::RateLimited),
            (StubFailure::Network, ProviderHealthStatus::Unreachable),
            (StubFailure::Timeout, ProviderHealthStatus::Unreachable),
            (
                StubFailure::Server,
                ProviderHealthStatus::ServerError { status: 0 },
            ),
        ] {
            let provider = FailingProvider { failure };
            let report = probe_provider_health(&provider, "failing", timeout).await;
            assert_eq!(report.status, expected, "report: {report:?}");
            assert!(report.error.is_some());
        }
    }

    #[tokio::test]
    async fn select_healthy_chat_prefers_first_available_and_records_fallback() {
        let host = StubHost {
            llm: HashMap::from([
                (
                    "openai".to_string(),
                    Arc::new(FailingProvider {
                        failure: StubFailure::Auth,
                    }) as Arc<dyn LlmProvider>,
                ),
                (
                    "anthropic".to_string(),
                    Arc::new(HealthyProvider) as Arc<dyn LlmProvider>,
                ),
            ]),
        };
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        let candidates = [
            candidate("primary", "openai"),
            candidate("backup", "anthropic"),
        ];
        let selection = select_healthy_chat(
            &candidates,
            &monitor,
            &host,
            &ene_config::EneConfig::default(),
            Duration::from_secs(1),
        )
        .await
        .expect("selection");
        assert!(selection.fell_back);
        assert_eq!(selection.candidate.provider, "backup");
        assert_eq!(selection.skipped.len(), 1);
        assert_eq!(selection.skipped[0].0, "primary");
        assert_eq!(monitor.fallback_history().len(), 1);
    }

    #[tokio::test]
    async fn select_healthy_chat_uses_cached_report_within_ttl() {
        let host = StubHost {
            llm: HashMap::from([(
                "openai".to_string(),
                Arc::new(HealthyProvider) as Arc<dyn LlmProvider>,
            )]),
        };
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        monitor.record(ProviderHealthReport {
            provider: "primary".to_string(),
            status: ProviderHealthStatus::AuthFailed,
            latency_ms: 5,
            error: Some("cached failure".to_string()),
            checked_at: Instant::now(),
        });
        let candidates = [candidate("primary", "openai")];
        let selection = select_healthy_chat(
            &candidates,
            &monitor,
            &host,
            &ene_config::EneConfig::default(),
            Duration::from_secs(1),
        )
        .await
        .expect("selection");
        // The cached failure is fresh, so the healthy stub is never probed;
        // the all-unhealthy fallback still returns the primary.
        assert_eq!(selection.candidate.provider, "primary");
        assert!(!selection.fell_back);
    }

    #[tokio::test]
    async fn select_healthy_chat_all_unhealthy_uses_primary_anyway() {
        let host = StubHost {
            llm: HashMap::from([(
                "openai".to_string(),
                Arc::new(FailingProvider {
                    failure: StubFailure::Network,
                }) as Arc<dyn LlmProvider>,
            )]),
        };
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        let candidates = [
            candidate("primary", "openai"),
            candidate("backup", "openai"),
        ];
        let selection = select_healthy_chat(
            &candidates,
            &monitor,
            &host,
            &ene_config::EneConfig::default(),
            Duration::from_secs(1),
        )
        .await
        .expect("selection");
        assert_eq!(selection.candidate.provider, "primary");
        assert_eq!(selection.skipped.len(), 2);
        assert!(!selection.fell_back);
    }

    #[tokio::test]
    async fn select_healthy_chat_reports_uncreatable_candidate_as_unavailable() {
        let host = StubHost {
            llm: HashMap::new(),
        };
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        let candidates = [candidate("primary", "not-a-plugin-kind")];
        let selection = select_healthy_chat(
            &candidates,
            &monitor,
            &host,
            &ene_config::EneConfig::default(),
            Duration::from_secs(1),
        )
        .await
        .expect("selection");
        assert_eq!(selection.candidate.provider, "primary");
        let report = monitor.get_any("primary").expect("report recorded");
        assert_eq!(report.status, ProviderHealthStatus::Unreachable);
        assert!(
            report
                .error
                .as_deref()
                .is_some_and(|d| d.contains("provider creation failed")),
            "detail: {:?}",
            report.error
        );
    }

    #[tokio::test]
    async fn select_healthy_chat_returns_none_without_candidates() {
        let host = StubHost {
            llm: HashMap::new(),
        };
        let monitor = ProviderHealthMonitor::new(Duration::from_mins(1), 8);
        assert!(
            select_healthy_chat(
                &[],
                &monitor,
                &host,
                &ene_config::EneConfig::default(),
                Duration::from_secs(1),
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn probe_chat_candidates_probes_every_candidate_fresh() {
        let host = StubHost {
            llm: HashMap::from([
                (
                    "openai".to_string(),
                    Arc::new(HealthyProvider) as Arc<dyn LlmProvider>,
                ),
                (
                    "anthropic".to_string(),
                    Arc::new(FailingProvider {
                        failure: StubFailure::Auth,
                    }) as Arc<dyn LlmProvider>,
                ),
            ]),
        };
        let candidates = [
            candidate("primary", "openai"),
            candidate("backup", "anthropic"),
        ];
        let reports = probe_chat_candidates(
            &candidates,
            &host,
            &ene_config::EneConfig::default(),
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].status, ProviderHealthStatus::Healthy);
        assert_eq!(reports[1].status, ProviderHealthStatus::AuthFailed);
    }
}
