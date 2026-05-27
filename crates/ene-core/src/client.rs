use async_openai::{Client, config::OpenAIConfig};

/// Builds an OpenAI-compatible client
pub fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig> {
    let mut config = OpenAIConfig::new().with_api_base(base_url);
    if !api_key.trim().is_empty() {
        config = config.with_api_key(api_key);
    }
    Client::with_config(config)
}
