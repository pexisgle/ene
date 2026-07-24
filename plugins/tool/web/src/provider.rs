use async_trait::async_trait;
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};
use ene_tool_common::{ActionSetProvider, ToolAction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Configuration for web search providers (Tavily, Exa).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "snake_case")]
pub struct WebSearchConfig {
    /// Tavily Search API Key
    pub tavily_api_key: String,
    /// Exa Search API Key
    pub exa_api_key: String,
}

fn generate_web_search_schema() -> serde_json::Value {
    let g = schemars::SchemaGenerator::default();
    let schema = g.into_root_schema_for::<WebSearchConfig>();
    // `schemars::Schema` is a thin wrapper around `serde_json::Value`. The
    // schemas produced by `into_root_schema_for` only contain JSON-safe
    // primitives, so the `Result` from `to_value` is only there for API
    // symmetry. A failure would mean a bug in schemars.
    #[expect(
        clippy::expect_used,
        reason = "schemars::Schema from into_root_schema_for is always JSON-serializable"
    )]
    serde_json::to_value(schema).expect("WebSearchConfig schema should always serialize")
}

/// Built-in web tool provider.
///
/// Exposes `webfetch` and `websearch` tools via [`ActionSetProvider`].
pub struct WebToolProvider {
    inner: ActionSetProvider,
}

impl WebToolProvider {
    /// Creates a new `WebToolProvider` and registers web actions.
    #[expect(
        clippy::expect_used,
        reason = "a default reqwest client silently loses the SSRF redirect policy, timeout, and user agent — failing fast is safer"
    )]
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
            // Disable automatic redirects entirely. The fetch action
            // follows redirects manually so it can re-run the SSRF
            // host check on every hop — a limited redirect policy
            // only caps hop count, not destination validation.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client builder should not fail");

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

        let config_for_set = config;
        let inner = ActionSetProvider::new(actions)
            .with_set_config_hook(move |value| {
                match serde_json::from_value::<WebSearchConfig>(value.clone()) {
                    Ok(cfg) => match config_for_set.write() {
                        Ok(mut guard) => *guard = cfg,
                        Err(e) => {
                            tracing::warn!("WebSearchConfig write lock poisoned: {e}");
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Ignoring malformed web search config: {e}");
                    }
                }
            })
            .with_config_schema_hook(|| Some(generate_web_search_schema()));

        Self { inner }
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
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_config(&self, config: &serde_json::Value) {
        self.inner.set_config(config);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        self.inner.config_schema()
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }
}
