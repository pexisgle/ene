use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "webfetch".to_string(),
        description: concat!(
            "Fetches content from a URL. ",
            "Returns the content in the requested format (text, markdown, or html). ",
            "Useful for reading documentation, APIs, or web pages."
        )
        .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to fetch content from (must start with http:// or https://)" },
                "format": { "type": "string", "description": "The format to return: text, markdown, or html. Defaults to markdown.", "enum": ["text", "markdown", "html"] },
                "timeout": { "type": "integer", "description": "Optional timeout in seconds (max 120)" }
            },
            "required": ["url"]
        }),
        category: Some(ToolCategory::Browser),
        keywords: vec![
            "fetch".to_string(),
            "url".to_string(),
            "web".to_string(),
            "download".to_string(),
            "html".to_string(),
        ],
    }
}

pub async fn webfetch(
    url: &str,
    format: Option<&str>,
    timeout: Option<u64>,
) -> Result<String, ToolError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolError::InvalidArguments {
            message: "URL must start with http:// or https://".to_string(),
        });
    }

    let timeout_secs = timeout
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS);
    let format = format.unwrap_or("markdown");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create HTTP client: {e}"),
        })?;

    let accept_header = match format {
        "text" => "text/plain;q=1.0, text/html;q=0.8, */*;q=0.1",
        "html" => "text/html;q=1.0, text/plain;q=0.8, */*;q=0.1",
        _ => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    };

    let response = client
        .get(url)
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
        "text" => Ok(crate::converter::html_to_text(&body)),
        "markdown" | _ => Ok(crate::converter::html_to_markdown(&body)),
    }
}
