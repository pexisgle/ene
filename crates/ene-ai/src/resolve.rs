//! Resolved provider settings from [`AiConfig`] task routing.

use crate::config::{
    AiConfig, AiProviderDef, ApiKeyConfig, LOCAL_PROVIDER, LocalModelDef, TaskRef,
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
    /// Local GGUF embedding via llama-cpp-2.
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
        }
    }
}

/// Validate chat-provider settings without performing a network call (#241).
///
/// Checks that the configured chat provider has a non-empty base URL with an
/// `http`/`https` scheme and a resolvable API key. Returns an empty vec when
/// the section is missing or the chat provider is local/GGUF.
#[must_use]
pub fn validate_settings(config: &ene_config::EneConfig) -> Vec<SettingsIssue> {
    let Ok(ai) = config.get_section::<crate::AiConfig>() else {
        return Vec::new();
    };
    let provider_key = ai.tasks.chat.provider.clone();
    if crate::AiConfig::is_local_provider(&provider_key) {
        return Vec::new();
    }
    let Some(def) = ai.providers.get(&provider_key) else {
        return vec![SettingsIssue::MissingBaseUrl {
            provider: provider_key,
        }];
    };
    // base_url / api_key are only meaningful for OpenAI-compatible HTTP
    // providers; plugin-provided kinds validate over IPC instead (#247).
    if !def.is_openai_compatible() {
        return Vec::new();
    }

    let mut issues = Vec::new();
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
}
