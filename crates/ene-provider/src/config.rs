use serde::{Deserialize, Serialize};

fn default_string() -> String {
    String::new()
}

/// Configuration for the local GGUF embedding backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
                    if let Ok(token) = std::env::var("API_TOKEN")
                        && !token.trim().is_empty()
                    {
                        return token;
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

    /// Reads the `local_embedding` sub-section from config.
    ///
    /// Falls back to `LocalEmbeddingConfig::default()` if the key is absent.
    #[must_use]
    pub fn local_embedding(config: &ene_config::EneConfig) -> LocalEmbeddingConfig {
        config
            .get_section_by_key::<LocalEmbeddingConfig>("provider.local_embedding")
            .unwrap_or_default()
    }
}
