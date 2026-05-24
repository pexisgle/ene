ene_config::define_label_enum!(
    pub enum EmbeddingProviderType {
        Api => "API (OpenAI-compatible)",
        #[default]
        Local => "Local (GGUF / Candle)",
    }
);

fn default_embedding_base_url() -> String {
    String::new()
}

ene_config::define_config!(
    "embedding",
    pub struct EmbeddingConfig {
        pub provider_type: EmbeddingProviderType = EmbeddingProviderType::Local,
        pub model: String = "jina-embeddings-v5-text-small".to_string(),
        pub base_url: String = default_embedding_base_url(),
        pub dimensions: Option<usize> = None,
        pub gguf_quantization: String = "F16".to_string(),
    }
);

impl EmbeddingConfig {
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
