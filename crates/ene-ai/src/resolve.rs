//! Resolved provider settings from [`AiConfig`] task routing.

use crate::config::{AiConfig, AiProviderDef, ApiKeyConfig, TaskRef};
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
    Local {
        /// Hub model name or identifier.
        model: String,
        /// Quantization format.
        quantization: String,
    },
}

impl ResolvedEmbedding {
    /// Cloud embedding fields (panics in debug if local).
    #[must_use]
    pub fn cloud_fields(&self) -> (&str, &str, &str, usize, Option<&str>) {
        match self {
            Self::Cloud {
                base_url,
                api_key,
                model,
                dimensions,
                query_prefix,
            } => (
                base_url.as_str(),
                api_key.as_str(),
                model.as_str(),
                *dimensions,
                query_prefix.as_deref(),
            ),
            Self::Local { .. } => {
                debug_assert!(false, "ResolvedEmbedding::cloud_fields on Local variant");
                ("", "", "", 0, None)
            }
        }
    }

    /// Local embedding fields (panics in debug if cloud).
    #[must_use]
    pub fn local_fields(&self) -> (&str, &str) {
        match self {
            Self::Local {
                model,
                quantization,
            } => (model.as_str(), quantization.as_str()),
            Self::Cloud { .. } => {
                debug_assert!(false, "ResolvedEmbedding::local_fields on Cloud variant");
                ("", "")
            }
        }
    }
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

/// Resolves an explicit or env-provided base URL for OpenAI-compatible APIs.
pub fn resolve_base_url(base_url: &str) -> Result<String, ene_config::ConfigError> {
    if !base_url.trim().is_empty() {
        return Ok(base_url.to_string());
    }
    if let Ok(url) = std::env::var("OPENAI_BASE_URL")
        && !url.trim().is_empty()
    {
        return Ok(url);
    }
    Err(ene_config::ConfigError::MissingBaseUrl {
        env_var: "OPENAI_BASE_URL".to_string(),
    })
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
            #[cfg(debug_assertions)]
            {
                if let Ok(token) = std::env::var("API_TOKEN")
                    && !token.trim().is_empty()
                {
                    return token;
                }
            }
            String::new()
        }
    }
}

impl AiConfig {
    /// Look up a named provider definition.
    pub fn get_provider(&self, name: &str) -> Result<&AiProviderDef, LlmProviderError> {
        self.providers
            .get(name)
            .ok_or_else(|| LlmProviderError::Provider(format!("unknown AI provider: {name:?}")))
    }

    /// Resolve a [`TaskRef`] to its provider and effective overrides.
    pub fn resolve_task_ref<'a>(
        &'a self,
        task: &'a TaskRef,
    ) -> Result<ResolvedTaskRef<'a>, LlmProviderError> {
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

    /// Resolve classifier chat settings (falls back to chat task).
    pub fn resolve_classifier(&self) -> Result<ResolvedChat, LlmProviderError> {
        self.resolve_chat_task(self.tasks.classifier.as_ref())
    }

    /// Resolve proactive generation chat settings (falls back to chat task).
    ///
    /// Generation must use an OpenAI-compatible provider; local GGUF is decision-only.
    pub fn resolve_proactive_generation(&self) -> Result<ResolvedChat, LlmProviderError> {
        if let Some(proactive) = self.tasks.proactive.as_ref()
            && matches!(
                self.get_provider(&proactive.provider)?,
                AiProviderDef::OpenaiCompatible { .. }
            )
        {
            return self.resolve_openai_chat_task(proactive);
        }
        self.resolve_chat()
    }

    /// Resolve embedding backend settings for [`AiConfig::tasks`] embedding task.
    pub fn resolve_embedding(&self) -> Result<ResolvedEmbedding, LlmProviderError> {
        let resolved = self.resolve_task_ref(&self.tasks.embedding)?;
        match resolved.provider {
            AiProviderDef::OpenaiCompatible { base_url, api_key } => {
                let model = resolved.model.ok_or_else(|| {
                    LlmProviderError::Provider(
                        "embedding task requires model for openai_compatible provider".to_string(),
                    )
                })?;
                let dimensions = resolved.dimensions.unwrap_or(1536);
                Ok(ResolvedEmbedding::Cloud {
                    base_url: resolve_base_url(base_url)
                        .map_err(|e| LlmProviderError::Provider(e.to_string()))?,
                    api_key: api_key.resolve_api_key(),
                    model: model.to_string(),
                    dimensions,
                    query_prefix: None,
                })
            }
            AiProviderDef::LocalGguf {
                model,
                quantization,
                ..
            } => Ok(ResolvedEmbedding::Local {
                model: model.clone(),
                quantization: quantization.clone(),
            }),
        }
    }

    fn resolve_openai_chat_task(&self, task: &TaskRef) -> Result<ResolvedChat, LlmProviderError> {
        let resolved = self.resolve_task_ref(task)?;
        match resolved.provider {
            AiProviderDef::OpenaiCompatible { base_url, api_key } => {
                let model = resolved.model.ok_or_else(|| {
                    LlmProviderError::Provider(format!(
                        "task for provider {:?} requires model",
                        task.provider
                    ))
                })?;
                Ok(ResolvedChat {
                    base_url: resolve_base_url(base_url)
                        .map_err(|e| LlmProviderError::Provider(e.to_string()))?,
                    api_key: api_key.resolve_api_key(),
                    model: model.to_string(),
                    max_tokens: resolved.max_tokens,
                })
            }
            AiProviderDef::LocalGguf { .. } => Err(LlmProviderError::Provider(
                "chat tasks cannot use local_gguf provider; use openai_compatible".to_string(),
            )),
        }
    }
}
