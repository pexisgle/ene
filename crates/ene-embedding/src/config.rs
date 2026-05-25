ene_config::define_label_enum!(
    /// Embedding backend provider type.
    pub enum EmbeddingProviderType {
        /// OpenAI-compatible remote API.
        Api => "API (OpenAI-compatible)",
        /// Local GGUF-quantized model via Candle.
        #[default]
        Local => "Local (GGUF / Candle)",
    }
);

fn default_embedding_base_url() -> String {
    String::new()
}

ene_config::define_config!(
    "embedding",
    /// Configuration for the embedding subsystem.
    pub struct EmbeddingConfig {
        /// Which embedding provider to use (API or local).
        pub provider_type: EmbeddingProviderType = EmbeddingProviderType::Local,
        /// Model name (e.g. `"jina-embeddings-v5-text-small"`).
        pub model: String = "jina-embeddings-v5-text-small".to_string(),
        /// Base URL for the API embedding provider (leave empty to inherit from provider settings).
        pub base_url: String = default_embedding_base_url(),
        /// Embedding dimensions (auto-detected for local models).
        pub dimensions: Option<usize> = None,
        /// GGUF quantization format (e.g. `"F16"`, `"Q4_K_M"`).
        pub gguf_quantization: String = "F16".to_string(),
    }
);

impl EmbeddingConfig {
    /// Resolves the effective base URL, falling back to the global provider settings.
    pub fn resolve_base_url(&self) -> Result<String, ene_config::ConfigError> {
        if !self.base_url.trim().is_empty() {
            return Ok(self.base_url.clone());
        }
        let settings = ene_config::get_global_settings();
        if let Some(base_url) = settings.extra.get("provider")
            .and_then(|p| p.get("base_url"))
            .and_then(|v| v.as_str())
        {
            if !base_url.trim().is_empty() {
                return Ok(base_url.to_string());
            }
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: String::new(),
        })
    }
}
