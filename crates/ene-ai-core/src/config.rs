fn default_string() -> String {
    String::new()
}

ene_config::define_config!(
    "provider",
    pub struct ProviderSettings {
        pub provider_name: String = "openai-compatible".to_string(),
        pub model: String = "gpt-4o-mini".to_string(),
        pub base_url: String = default_string(),
        pub api_key: String = default_string(),
    }
);

impl ProviderSettings {
    pub fn resolve_base_url(&self) -> Result<String, ene_config::ConfigError> {
        if !self.base_url.trim().is_empty() {
            return Ok(self.base_url.clone());
        }
        Err(ene_config::ConfigError::MissingBaseUrl {
            env_var: String::new(),
        })
    }

    pub fn resolve_api_key(&self) -> String {
        if !self.api_key.trim().is_empty() {
            return self.api_key.clone();
        }
        #[cfg(debug_assertions)]
        {
            if let Ok(token) = std::env::var("API_TOKEN") {
                if !token.trim().is_empty() {
                    return token;
                }
            }
        }
        String::new()
    }
}
