use ene_tool_common::prelude::*;

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;

fn default_client() -> reqwest::Client {
    reqwest::Client::new()
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "web",
    name = "fetch",
    summary = "Fetch a URL and return its content as text or markdown.",
    description = "Fetches content from a URL and returns it in the requested format (markdown, text, or html). Supports configurable timeout and automatically converts HTML to readable markdown.",
    category = "WebFetch",
    keywords_primary = "fetch, url, web, download, html"
)]
pub struct WebFetchAction {
    #[tool(skip)]
    #[serde(skip, default = "default_client")]
    client: reqwest::Client,
    /// The URL to fetch content from (must start with http:// or https://).
    url: String,
    /// The format to return: text, markdown, or html. Defaults to markdown.
    #[arg(enum_values = "text, markdown, html", default = "markdown")]
    #[serde(default)]
    format: Option<String>,
    /// Optional timeout in seconds (max 120).
    #[arg(minimum = 1, maximum = 120, default = "30")]
    #[serde(default)]
    timeout: Option<u64>,
}

impl WebFetchAction {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            url: String::new(),
            format: None,
            timeout: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let url = &self.url;
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::InvalidArguments {
                message: "URL must start with http:// or https://".to_string(),
            });
        }

        let timeout_secs = self
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);
        let format = self.format.as_deref().unwrap_or("markdown");

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
        if let Some(len) = content_length
            && len > MAX_RESPONSE_SIZE as u64
        {
            return Err(ToolError::ExecutionFailed {
                message: "Response too large (exceeds 5MB limit)".to_string(),
            });
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
            _ => Ok(ene_tool_common::html::html_to_markdown(&body)),
        }
    }
}
