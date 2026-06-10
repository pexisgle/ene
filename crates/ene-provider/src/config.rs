fn default_string() -> String {
    String::new()
}

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
        /// `"local"` uses a local GGUF model via `ene-embedding`.
        pub backend: String = "cloud".to_string(),
        /// Optional query prefix to prepend to search queries (e.g. "Query: ").
        pub query_prefix: Option<String> = None,
        /// Cloud embedding configuration.
        pub cloud: CloudEmbeddingConfig,
        /// Local embedding configuration.
        pub local: LocalEmbeddingConfig,
    }
);

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
        /// API key configuration.
        pub api_key: ApiKeyConfig,
        /// Embedding configuration.
        pub embedding: EmbeddingConfig,
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

    /// Resolves the API key from the configured source (inline or env).
    pub fn resolve_api_key(&self) -> String {
        match self.api_key.source.as_str() {
            "inline" => {
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
            "env" => {
                let var_name = if self.api_key.env.trim().is_empty() {
                    "OPENAI_API_KEY"
                } else {
                    self.api_key.env.trim()
                };
                std::env::var(var_name).unwrap_or_default()
            }
            _ => {
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
    }
}
