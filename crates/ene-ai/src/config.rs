const fn default_string() -> String {
    String::new()
}

use ene_config::schemars;

ene_config::define_config!(
    ProviderConfig,
    "api_key",
    /// Configuration for API key retrieval.
    pub struct ApiKeyConfig {
        /// Key source: `"inline"` or `"env"`.
        pub source: String = "inline".to_string(),
        /// API key (inline — use with caution).
        pub inline: String = default_string(),
        /// Environment variable name when `source = "env"`.
        pub env: String = "OPENAI_API_KEY".to_string(),
    }
);

ene_config::define_config!(
    EmbeddingConfig,
    "cloud",
    /// Configuration for the cloud embedding backend.
    pub struct CloudEmbeddingConfig {
        /// Cloud embedding model name (used when `backend = "cloud"`).
        pub model: String = "text-embedding-3-small".to_string(),
        /// Expected dimensions for cloud embedding vectors.
        pub dimensions: usize = 1536,
    }
);

ene_config::define_config!(
    EmbeddingConfig,
    "local",
    /// Configuration for the local GGUF embedding backend.
    pub struct LocalEmbeddingConfig {
        /// Local GGUF embedding model name (e.g. `"jina-embeddings-v5-text-small"`).
        pub model: String = "jina-embeddings-v5-text-small".to_string(),
        /// Quantization level (e.g. `"F16"`, `"Q4_K_M"`).
        pub quantization: String = "F16".to_string(),
    }
);

ene_config::define_config!(
    ProviderConfig,
    "embedding",
    /// Configuration for the embedding system.
    pub struct EmbeddingConfig {
        /// Embedding backend: `"cloud"` uses the same provider's embedding API;
        /// `"local"` uses a local GGUF model via `ene-ai`.
        pub backend: String = "cloud".to_string(),
        /// Optional query prefix to prepend to search queries (e.g. "Query: ").
        pub query_prefix: Option<String> = None,
        /// Cloud embedding configuration.
        pub cloud: CloudEmbeddingConfig,
        /// Local embedding configuration.
        pub local: LocalEmbeddingConfig,
    }
);

ene_config::define_label_enum!(
    /// Backend used for proactive speech decisions (#103 / #165 / #171).
    pub enum ProactiveDecisionBackend {
        /// In-process llama-cpp-2 (GGUF on `model_path`).
        LlamaCpp => "llama_cpp",
        /// Existing cloud OpenAI-compatible provider with an optional model override.
        Cloud => "cloud",
        /// Decision path disabled (no proactive speech).
        #[default]
        Disabled => "disabled",
    }
);

ene_config::define_label_enum!(
    /// GPU / CPU acceleration preference for local llama.cpp (#165).
    pub enum ProactiveAcceleration {
        /// Pick from OS / available backends / startup result.
        #[default]
        Auto => "auto",
        /// Force Vulkan (AMD RADV / cross-vendor Vulkan).
        Vulkan => "vulkan",
        /// Force CUDA.
        Cuda => "cuda",
        /// CPU only.
        Cpu => "cpu",
    }
);

ene_config::define_label_enum!(
    /// Fallback when the local decision backend fails (#165).
    pub enum ProactiveDecisionFallback {
        /// Do not speak; never silently upload to cloud.
        #[default]
        Disabled => "disabled",
        /// Retry decision via the cloud OpenAI-compatible provider.
        Cloud => "cloud",
    }
);

/// Local / cloud decision model settings for proactive speech (#103 / #165 / #171).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveDecisionProviderConfig {
    /// Decision backend: `llama_cpp`, `cloud`, or `disabled`.
    pub backend: ProactiveDecisionBackend,
    /// Absolute or relative path to the decision GGUF weights.
    pub model_path: String,
    /// Preferred acceleration backend.
    pub acceleration: ProactiveAcceleration,
    /// GPU layer offload: `"auto"` or an integer string (e.g. `"33"`).
    pub gpu_layers: String,
    /// Context size for the decision model (small is preferred).
    pub context_size: u32,
    /// Bounded wait for local GGUF model load.
    pub startup_timeout_seconds: u64,
    /// Per-request timeout for decision completion.
    pub request_timeout_seconds: u64,
    /// What to do when the local backend fails.
    pub fallback: ProactiveDecisionFallback,
    /// Optional cloud model override when `backend` or `fallback` is cloud.
    pub cloud_model: String,
}

impl Default for ProactiveDecisionProviderConfig {
    fn default() -> Self {
        Self {
            backend: ProactiveDecisionBackend::Disabled,
            model_path: String::new(),
            acceleration: ProactiveAcceleration::Auto,
            gpu_layers: "auto".to_string(),
            context_size: 2048,
            startup_timeout_seconds: 60,
            request_timeout_seconds: 20,
            fallback: ProactiveDecisionFallback::Disabled,
            cloud_model: String::new(),
        }
    }
}

/// Proactive speech model routing under `provider.proactive` (#103).
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveProviderConfig {
    /// Lightweight decision model settings.
    pub decision: ProactiveDecisionProviderConfig,
    /// Optional generation model override (empty = use `provider.model`).
    pub generation_model: String,
}

ene_config::define_config!(
    settings,
    "provider",
    /// AI provider connection config, including embedding backend settings.
    pub struct ProviderConfig {
        /// Provider name (e.g. `"openai-compatible"`).
        pub name: String = "openai-compatible".to_string(),
        /// Chat model name (e.g. `"gpt-4o-mini"`).
        pub model: String = "gpt-4o-mini".to_string(),
        /// API base URL.
        pub base_url: String = default_string(),
        /// Max completion tokens for chat (`0` = omit; providers like OpenRouter
        /// then reserve the model max, which can trigger HTTP 402 on low balance).
        pub max_tokens: u32 = 8192,
        /// API key configuration.
        pub api_key: ApiKeyConfig,
        /// Embedding configuration.
        pub embedding: EmbeddingConfig,
        /// Proactive companion speech model routing (#103).
        pub proactive: ProactiveProviderConfig,
    }
);

impl ProviderConfig {
    /// Resolves the effective base URL, falling back to defaults.
    pub fn resolve_base_url(&self) -> Result<String, ene_config::ConfigError> {
        if !self.base_url.trim().is_empty() {
            return Ok(self.base_url.clone());
        }
        // Fall back to OPENAI_BASE_URL env var for cloud API providers.
        if let Ok(url) = std::env::var("OPENAI_BASE_URL")
            && !url.trim().is_empty()
        {
            return Ok(url);
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: "OPENAI_BASE_URL".to_string(),
        })
    }

    /// Resolves the API key from the configured source (inline or env).
    pub fn resolve_api_key(&self) -> String {
        if self.api_key.source.as_str() == "env" {
            let var_name = if self.api_key.env.trim().is_empty() {
                "OPENAI_API_KEY"
            } else {
                self.api_key.env.trim()
            };
            std::env::var(var_name).unwrap_or_default()
        } else {
            if !self.api_key.inline.trim().is_empty() {
                return self.api_key.inline.clone();
            }
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

    /// Effective chat model for proactive generation (empty override → `model`).
    #[must_use]
    pub fn proactive_generation_model(&self) -> &str {
        let override_model = self.proactive.generation_model.trim();
        if override_model.is_empty() {
            self.model.as_str()
        } else {
            override_model
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proactive_provider_defaults_are_disabled() {
        let cfg = ProviderConfig::default();
        assert_eq!(
            cfg.proactive.decision.backend,
            ProactiveDecisionBackend::Disabled
        );
        assert!(cfg.proactive.generation_model.is_empty());
        assert_eq!(cfg.proactive_generation_model(), "gpt-4o-mini");
    }

    #[test]
    fn proactive_generation_model_override() {
        let mut cfg = ProviderConfig::default();
        cfg.proactive.generation_model = "gpt-4o".to_string();
        assert_eq!(cfg.proactive_generation_model(), "gpt-4o");
    }

    #[test]
    fn proactive_decision_backend_deserializes() {
        let cfg: ProactiveDecisionProviderConfig =
            serde_json::from_str(r#"{"backend":"llama_cpp"}"#).expect("deserialize");
        assert_eq!(cfg.backend, ProactiveDecisionBackend::LlamaCpp);
    }
}
