use crate::config::WebSearchConfig;
use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use std::sync::OnceLock;

fn generate_web_search_schema() -> serde_json::Value {
    use schemars::r#gen::SchemaSettings;
    let g = SchemaSettings::draft07().into_generator();
    let schema = g.into_root_schema_for::<WebSearchConfig>();
    serde_json::to_value(schema).expect("WebSearchConfig schema should always serialize")
}

pub struct WebToolProvider {
    client: reqwest::Client,
    config: OnceLock<WebSearchConfig>,
}

impl WebToolProvider {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".parse().unwrap());
                headers.insert(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.5".parse().unwrap());
                headers
            })
            .build()
            .unwrap_or_default();
        Self {
            client,
            config: OnceLock::new(),
        }
    }
}

#[derive(serde::Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

#[derive(serde::Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolProvider for WebToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            crate::webfetch::tool_definition(),
            crate::websearch::tool_definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "webfetch" => {
                let args: WebFetchArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Invalid arguments for webfetch: {e}"),
                    })?;
                crate::webfetch::webfetch(
                    &self.client,
                    &args.url,
                    args.format.as_deref(),
                    args.timeout,
                )
                .await
            }
            "websearch" => {
                let args: WebSearchArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: format!("Invalid arguments for websearch: {e}"),
                    })?;
                crate::websearch::websearch(
                    &args.query,
                    args.backend.as_deref(),
                    args.limit,
                    self.config.get(),
                )
                .await
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_config(&self, config: &serde_json::Value) {
        if let Ok(cfg) = serde_json::from_value::<WebSearchConfig>(config.clone()) {
            let _ = self.config.set(cfg);
        }
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        Some(generate_web_search_schema())
    }

    fn set_session_id(&self, _session_id: &str) {
        // Web tools are stateless
    }
}
