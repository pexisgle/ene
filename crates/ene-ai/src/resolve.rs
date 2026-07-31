//! Resolved provider settings from [`AiConfig`] task routing.

use crate::config::{
    AiConfig, AiProviderDef, ApiKeyConfig, LOCAL_PROVIDER, LocalModelDef, TaskRef,
    kind_typo_suggestion,
};
use crate::error::LlmProviderError;

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
}

impl ResolvedLocalModel {
    pub(crate) fn from_named(name: &str, def: &LocalModelDef) -> Self {
        Self {
            name: name.to_string(),
            url: def.url.clone(),
            model_path: def.model_path.clone(),
            mmproj_url: def.mmproj_url.clone(),
            mmproj_path: def.mmproj_path.clone(),
            quantization: def.quantization.clone(),
            acceleration: def.acceleration,
            gpu_layers: def.gpu_layers.clone(),
            context_size: def.context_size,
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
    #[must_use]
    pub fn cloud_fields(&self) -> Option<(&str, &str, &str, usize, Option<&str>)> {
        match self {
            Self::Cloud {
                base_url,
                api_key,
                model,
                dimensions,
                query_prefix,
            } => Some((
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
    /// Model name (provider-specific).
    pub model: String,
    /// Speech probability threshold, clamped to `[0.0, 1.0]`.
    pub threshold: f32,
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

/// A fully resolved cloud chat candidate for failover routing (#175).
///
/// Produced by [`AiConfig::resolve_chat_candidates`], which enumerates the
/// configured chat provider first (highest priority) followed by every other
/// cloud provider in [`AiConfig::providers`] order. The runtime probes each
/// candidate's health and selects the first available one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCandidate {
    /// Provider name (key in [`AiConfig::providers`]).
    pub provider: String,
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

/// A single settings validation finding (#241).
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
    /// Provider `kind` looks like a typo of a built-in kind (#C3).
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

/// Validate provider `kind` values against known built-ins (#C3).
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

/// Validate chat-provider settings without performing a network call (#241).
///
/// Checks that the configured chat provider has a non-empty base URL with an
/// `http`/`https` scheme and a resolvable API key. Returns an empty vec when
/// the section is missing or the chat provider is local/GGUF. Also reports
/// provider `kind` values that look like typos of built-in kinds (#C3).
#[must_use]
pub fn validate_settings(config: &ene_config::EneConfig) -> Vec<SettingsIssue> {
    let Ok(ai) = config.get_section::<crate::AiConfig>() else {
        return Vec::new();
    };

    // Validate every provider's `kind` up front, independent of which provider
    // the chat task routes to (#C3).
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
    // providers; plugin-provided kinds validate over IPC instead (#247).
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
/// output reserve (#366).
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
/// budget plus output reserve (#366).
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
/// a [`ContextBudgetIssue`] when the window is too small (#366).
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

/// Emit a `tracing::warn!` for each under-sized context window (#366).
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
}

/// Lightweight API-key validation for an OpenAI-compatible provider (#237).
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

    /// Compute the effective context-window budget for a task (#364).
    ///
    /// `provider_advertised` is the window the backend reports for itself —
    /// [`ene_plugin_proto::LlmProviderSpec::context_window`] for a plugin
    /// provider, or `Some(LocalModelDef.context_size)` for a local model. It
    /// is combined with any `ai.providers.<name>.context_window` override on
    /// the task's provider (as `min`, so config can only shrink the model's
    /// stated limit) and the task's `max_tokens` response reserve, via
    /// [`crate::context_window::effective_window`]. The safety margin uses
    /// the heuristic default; callers with measured usage (#365) can recompute
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

    /// Resolve an ordered list of cloud chat candidates for failover (#175).
    ///
    /// The configured chat provider is first (highest priority), followed by
    /// every other cloud provider in [`AiConfig::providers`] insertion order.
    /// Local providers are excluded (chat requires an OpenAI-compatible API).
    /// Candidates that fail to resolve (missing base URL / model) are skipped.
    #[must_use]
    pub fn resolve_chat_candidates(&self) -> Vec<ChatCandidate> {
        let mut candidates = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Primary: the configured chat task provider.
        if let Ok(resolved) = self.resolve_chat()
            && seen.insert(self.tasks.chat.provider.clone())
        {
            candidates.push(ChatCandidate {
                provider: self.tasks.chat.provider.clone(),
                base_url: resolved.base_url,
                api_key: resolved.api_key,
                model: resolved.model,
                max_tokens: resolved.max_tokens,
            });
        }

        // Fallbacks: every other OpenAI-compatible provider, in config order.
        // Only HTTP providers are health-probed here; plugin-provided kinds
        // are checked over IPC instead (#247).
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
                "embedding provider {:?} has kind {:?}; only openai_compatible providers are supported here (plugin providers resolve via the plugin registry)",
                self.tasks.embedding.provider, def.kind
            )));
        }
        let model = resolved.model.ok_or_else(|| {
            LlmProviderError::Provider(
                "embedding task requires model for openai_compatible provider".to_string(),
            )
        })?;
        let dimensions = resolved.dimensions.unwrap_or(1536);
        Ok(ResolvedEmbedding::Cloud {
            base_url: resolve_base_url(&def.base_url)?,
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
                "chat tasks cannot use local provider; use openai_compatible".to_string(),
            ));
        }
        let resolved = self.resolve_task_ref(task)?;
        let def = resolved.provider;
        if !def.is_openai_compatible() {
            return Err(LlmProviderError::Provider(format!(
                "chat provider {:?} has kind {:?}; only openai_compatible providers are supported here (plugin providers resolve via the plugin registry)",
                task.provider, def.kind
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
    /// Returns `None` when the provider is `"none"` (VAD disabled). The
    /// `threshold` is clamped to `[0.0, 1.0]` with a warning when adjusted.
    #[must_use]
    pub fn resolve_vad(&self) -> Option<ResolvedVad> {
        if self.vad.provider == "none" {
            return None;
        }
        let threshold = clamp_with_warn(self.vad.threshold, 0.0, 1.0, "ai.vad.threshold");
        Some(ResolvedVad {
            provider: self.vad.provider.clone(),
            model: self.vad.model.clone(),
            threshold,
        })
    }
}

/// Clamp `value` to `[min, max]`, emitting a `tracing::warn!` when it is
/// adjusted. Used to sanitize numeric config values at the resolve boundary
/// (M12).
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolve_vad_clamps_threshold() {
        let mut cfg = AiConfig::default();
        cfg.vad.provider = "silero".to_string();
        cfg.vad.threshold = 2.5;

        let resolved = cfg.resolve_vad().expect("vad enabled");
        assert_eq!(resolved.provider, "silero");
        assert!((resolved.threshold - 1.0).abs() < f32::EPSILON);
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
        // Operator override (32k) caps the advertised 200k window.
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

    /// Build an [`AiConfig`] whose chat and proactive tasks both route to a
    /// local model with the given context size (#366 test helper).
    fn local_chat_config(context_size: u32) -> AiConfig {
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
            max_tokens: Some(8_192),
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
        // 2,048-token window cannot hold a 12,000 prompt + 8,192 reserve.
        let cfg = local_chat_config(2_048);
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
        assert_eq!(chat.response_reserve, 8_192);
        assert_eq!(chat.required, 20_192);
        // The message carries the current value, required value, and breakdown.
        let msg = chat.message();
        assert!(msg.contains("2048"), "message: {msg}");
        assert!(msg.contains("20192"), "message: {msg}");
        assert!(msg.contains("12000"), "message: {msg}");
        assert!(msg.contains("8192"), "message: {msg}");
    }

    #[test]
    fn validate_context_budgets_passes_with_default_local_window() {
        // The raised 16,384 default holds a 12,000 prompt + small reserve.
        let cfg = local_chat_config(16_384);
        assert!(validate_context_budgets(&cfg, 12_000).is_empty());
    }

    #[test]
    fn validate_context_budgets_skips_cloud_without_override() {
        // A cloud chat task with no explicit window is learned at runtime, so
        // it is not flagged on the conservative default floor.
        let cfg = AiConfig::default();
        assert!(validate_context_budgets(&cfg, 12_000).is_empty());
    }

    #[test]
    fn validate_context_budgets_flags_small_cloud_override() {
        // An operator override that shrinks the window below the budget is a
        // startup-knowable misconfiguration and is flagged.
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
        // Only generative tasks (chat, proactive) are checked; a tiny
        // embedding/classifier window must not produce an issue.
        let mut cfg = local_chat_config(16_384);
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
}
