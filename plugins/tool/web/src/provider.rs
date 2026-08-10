use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{ToolError, ToolProvider, ToolSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

use crate::broker::WebBroker;

/// Names of host-owned credentials used by web search providers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "snake_case")]
pub struct WebSearchConfig {
    /// Host credential name for Tavily Search.
    #[serde(default = "default_tavily_credential")]
    pub tavily_credential: String,
    /// Host credential name for Exa Search.
    #[serde(default = "default_exa_credential")]
    pub exa_credential: String,
}

fn default_tavily_credential() -> String {
    "tavily_api_key".to_string()
}

fn default_exa_credential() -> String {
    "exa_api_key".to_string()
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            tavily_credential: default_tavily_credential(),
            exa_credential: default_exa_credential(),
        }
    }
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
    pub fn new() -> Self {
        let broker = WebBroker::new();

        // RwLock so the API key can be hot-reloaded by a reconfigure
        // without restarting the tool binary; a `OnceLock` would only
        // honor the first `set_config`, so updating the search API key
        // in settings would require restarting the process.
        let config = Arc::new(RwLock::new(WebSearchConfig::default()));

        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(crate::action::WebFetchAction::new(Arc::clone(&broker))),
            Box::new(crate::action::WebSearchAction::new(
                config.clone(),
                Arc::clone(&broker),
            )),
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

        let broker_for_sandbox = broker;
        let inner = inner.with_sandbox_hook(move |sandbox| {
            broker_for_sandbox.configure(sandbox);
        });

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
