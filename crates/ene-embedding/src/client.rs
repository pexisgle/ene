use async_openai::{Client, config::OpenAIConfig};

/// Builds an OpenAI-compatible client with the given base URL and API key.
pub fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig> {
    let mut config = OpenAIConfig::default().with_api_key(api_key);
    if !base_url.is_empty() {
        config = config.with_api_base(base_url);
    }
    Client::with_config(config)
}
