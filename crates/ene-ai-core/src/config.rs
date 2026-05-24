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
        pub api_key_source: String = "inline".to_string(),
        pub api_key_env: String = "OPENAI_API_KEY".to_string(),
        pub api_key_keyring_service: String = "dev.pexisgle.Ene".to_string(),
        pub api_key_keyring_account: String = "default".to_string(),
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
        match self.api_key_source.as_str() {
            "inline" => {
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
            "env" => {
                let var_name = if self.api_key_env.trim().is_empty() {
                    "OPENAI_API_KEY"
                } else {
                    self.api_key_env.trim()
                };
                std::env::var(var_name).unwrap_or_default()
            }
            "keyring" => {
                let service = if self.api_key_keyring_service.trim().is_empty() {
                    "dev.pexisgle.Ene"
                } else {
                    self.api_key_keyring_service.trim()
                };
                let account = if self.api_key_keyring_account.trim().is_empty() {
                    "default"
                } else {
                    self.api_key_keyring_account.trim()
                };
                match keyring::Entry::new(service, account) {
                    Ok(entry) => entry.get_password().unwrap_or_default(),
                    Err(e) => {
                        tracing::error!("Failed to retrieve password from keyring: {}", e);
                        String::new()
                    }
                }
            }
            _ => {
                // Fallback / legacy behavior
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
    }
}
