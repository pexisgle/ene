use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Configuration for web search providers (Tavily, Brave, Exa).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "snake_case")]
pub struct WebSearchConfig {
    /// Tavily Search API Key
    pub tavily_api_key: String,
    /// Brave Search API Key
    pub brave_api_key: String,
    /// Exa Search API Key
    pub exa_api_key: String,
}

fn generate_web_search_schema() -> serde_json::Value {
    let g = schemars::SchemaGenerator::default();
    let schema = g.into_root_schema_for::<WebSearchConfig>();
    serde_json::to_value(schema).expect("WebSearchConfig schema should always serialize")
}

/// Built-in web tool provider.
///
/// Exposes `webfetch` and `websearch` tools.
/// Internally uses a dynamic list of actions implementing `ToolAction`.
pub struct WebToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
    config: Arc<RwLock<WebSearchConfig>>,
}

impl WebToolProvider {
    /// Creates a new `WebToolProvider` and registers web actions.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    reqwest::header::ACCEPT,
                    reqwest::header::HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
                );
                headers.insert(
                    reqwest::header::ACCEPT_LANGUAGE,
                    reqwest::header::HeaderValue::from_static("en-US,en;q=0.5"),
                );
                headers
            })
            // Limit the redirect chain so an SSRF-by-redirect (target
            // is a public host that 30x's to a private one) cannot
            // smuggle a fetch past the per-host SSRF check.
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_default();

        // RwLock so the API key can be hot-reloaded by a
        // reconfigure without restarting the tool
        // binary. The previous `OnceLock` only allowed
        // the first `set_config` to take effect; a
        // user updating their search API key in
        // settings would have to bounce the entire
        // process for the new key to be picked up.
        let config = Arc::new(RwLock::new(WebSearchConfig::default()));

        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(crate::action::WebFetchAction::new(client)),
            Box::new(crate::action::WebSearchAction::new(config.clone())),
        ];

        Self { actions, config }
    }
}

impl Default for WebToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for WebToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.actions.iter().map(|a| a.definition()).collect()
    }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_config(&self, config: &serde_json::Value) {
        if let Ok(cfg) = serde_json::from_value::<WebSearchConfig>(config.clone())
            && let Ok(mut guard) = self.config.write()
        {
            *guard = cfg;
        }
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        Some(generate_web_search_schema())
    }

    fn set_session_id(&self, _session_id: &str) {
        // Web tools are stateless
    }
}
