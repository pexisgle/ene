pub mod provider;
pub mod webfetch;
pub mod websearch;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "snake_case")]
pub struct WebSearchConfig {
    pub tavily_api_key: String,
    pub google_api_key: String,
    pub google_cse_cx: String,
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
        assert_eq!(cfg.google_api_key, "");
        assert_eq!(cfg.google_cse_cx, "");

        let cfg: WebSearchConfig = serde_json::from_value(serde_json::json!({
            "google_api_key": "g-key",
            "google_cse_cx": "cx-val"
        }))
        .unwrap();
        assert_eq!(cfg.tavily_api_key, "");
        assert_eq!(cfg.google_api_key, "g-key");
        assert_eq!(cfg.google_cse_cx, "cx-val");
    }
}
