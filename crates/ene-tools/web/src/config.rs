use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "snake_case")]
pub struct WebSearchConfig {
    pub tavily_api_key: String,
    pub brave_api_key: String,
    pub exa_api_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_config_deserialize_partial_succeeds() {
        let cfg: WebSearchConfig = serde_json::from_value(serde_json::json!({
            "tavily_api_key": "test-key"
        }))
        .unwrap();
        assert_eq!(cfg.tavily_api_key, "test-key");
    }
}
