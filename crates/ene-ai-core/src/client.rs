use async_openai::{config::OpenAIConfig, Client};

/// OpenAI互換クライアントを構築する
pub fn build_openai_client(base_url: &str, api_key: &str) -> Client<OpenAIConfig> {
    let mut config = OpenAIConfig::new().with_api_base(base_url);
    if !api_key.trim().is_empty() {
        config = config.with_api_key(api_key);
    }
    Client::with_config(config)
}
