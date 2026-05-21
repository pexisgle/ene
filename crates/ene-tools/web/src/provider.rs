use ene_tool_proto::{ToolDefinition, ToolError, ToolProvider};
use async_trait::async_trait;

pub struct WebToolProvider;

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
                let args: WebFetchArgs = serde_json::from_str(arguments).map_err(|e| {
                    ToolError::InvalidArguments {
                        message: format!("Invalid arguments for webfetch: {e}"),
                    }
                })?;
                crate::webfetch::webfetch(&args.url, args.format.as_deref(), args.timeout).await
            }
            "websearch" => {
                let args: WebSearchArgs = serde_json::from_str(arguments).map_err(|e| {
                    ToolError::InvalidArguments {
                        message: format!("Invalid arguments for websearch: {e}"),
                    }
                })?;
                crate::websearch::websearch(&args.query, args.backend.as_deref(), args.limit).await
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {
        // Web tools are stateless
    }
}
