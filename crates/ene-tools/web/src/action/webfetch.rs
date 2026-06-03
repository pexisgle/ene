use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use serde::Deserialize;

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;

#[derive(Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

/// Action to fetch content from a URL.
pub struct WebFetchAction {
    client: reqwest::Client,
}

impl WebFetchAction {
    /// Creates a new `WebFetchAction` with a given HTTP client.
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolAction for WebFetchAction {
    fn tool_name(&self) -> &'static str {
        "web.fetch"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("web.fetch"),
            version: ToolVersion::default(),
            display_name: "Fetch a URL and return its content as text or markdown.".to_string(),
            summary: "Fetch a URL and return its content as text or markdown.".to_string(),
            description: "Fetches content from a URL and returns it in the requested format (markdown, text, or html). Supports configurable timeout and automatically converts HTML to readable markdown.".to_string(),
            category: ToolCategory::WebFetch,
            keywords: KeywordSet::primary_only(["fetch", "url", "web", "download", "html"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to fetch content from (must start with http:// or https://)" },
                    "format": { "type": "string", "description": "The format to return: text, markdown, or html. Defaults to markdown.", "enum": ["text", "markdown", "html"] },
                    "timeout": { "type": "integer", "description": "Optional timeout in seconds (max 120)" }
                },
                "required": ["url"]
            }),
            examples: vec![
                ToolExample {
                    description: "Fetch a webpage as markdown".to_string(),
                    input: serde_json::json!({"url": "https://example.com"}),
                    output: Some("Content of example.com converted to readable markdown".to_string()),
                },
                ToolExample {
                    description: "Fetch with text format and custom timeout".to_string(),
                    input: serde_json::json!({"url": "https://api.example.com/data", "format": "text", "timeout": 15}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: WebFetchArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for webfetch: {e}"),
            })?;

        let url = &args.url;
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::InvalidArguments {
                message: "URL must start with http:// or https://".to_string(),
            });
        }

        let timeout_secs = args
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);
        let format = args.format.as_deref().unwrap_or("markdown");

        let accept_header = match format {
            "text" => "text/plain;q=1.0, text/html;q=0.8, */*;q=0.1",
            "html" => "text/html;q=1.0, text/plain;q=0.8, */*;q=0.1",
            _ => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        };

        let response = self
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .header("Accept", accept_header)
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("HTTP request failed: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "HTTP request returned status: {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ),
            });
        }

        let content_length = response.content_length();
        if let Some(len) = content_length {
            if len > MAX_RESPONSE_SIZE as u64 {
                return Err(ToolError::ExecutionFailed {
                    message: "Response too large (exceeds 5MB limit)".to_string(),
                });
            }
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to read response body: {e}"),
            })?;

        if bytes.len() > MAX_RESPONSE_SIZE {
            return Err(ToolError::ExecutionFailed {
                message: "Response too large (exceeds 5MB limit)".to_string(),
            });
        }

        let body = String::from_utf8_lossy(&bytes);

        match format {
            "html" => Ok(body.to_string()),
            "text" | "markdown" | _ => Ok(ene_tools_common::html::html_to_markdown(&body)),
        }
    }
}
