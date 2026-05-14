use crate::error::AiCoreError;
use super::definition::ToolDefinition;

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
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to fetch content from (must start with http:// or https://)" },
                "format": { "type": "string", "description": "The format to return: text, markdown, or html. Defaults to markdown.", "enum": ["text", "markdown", "html"] },
                "timeout": { "type": "integer", "description": "Optional timeout in seconds (max 120)" }
            },
            "required": ["url"]
        }),
    }
}

/// URLからコンテンツを取得
pub async fn webfetch(
    url: &str,
    format: Option<&str>,
    timeout: Option<u64>,
) -> Result<String, AiCoreError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AiCoreError::ToolExecutionError(
            "URL must start with http:// or https://".to_string()
        ));
    }

    let timeout_secs = timeout.unwrap_or(DEFAULT_TIMEOUT_SECS).min(MAX_TIMEOUT_SECS);
    let format = format.unwrap_or("markdown");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| AiCoreError::ToolExecutionError(format!("Failed to create HTTP client: {e}")))?;

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
        .map_err(|e| AiCoreError::ToolExecutionError(format!("HTTP request failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AiCoreError::ToolExecutionError(format!(
            "HTTP request returned status: {} {}", status.as_u16(), status.canonical_reason().unwrap_or("Unknown")
        )));
    }

    let content_length = response.content_length();
    if let Some(len) = content_length {
        if len > MAX_RESPONSE_SIZE as u64 {
            return Err(AiCoreError::ToolExecutionError(
                "Response too large (exceeds 5MB limit)".to_string()
            ));
        }
    }

    let bytes = response.bytes().await
        .map_err(|e| AiCoreError::ToolExecutionError(format!("Failed to read response body: {e}")))?;

    if bytes.len() > MAX_RESPONSE_SIZE {
        return Err(AiCoreError::ToolExecutionError(
            "Response too large (exceeds 5MB limit)".to_string()
        ));
    }

    let _content_type = match format {
        "html" => "text/html",
        "text" => "text/plain",
        _ => "text/html", // default to html for markdown conversion
    };

    let body = String::from_utf8_lossy(&bytes);

    match format {
        "html" => Ok(body.to_string()),
        "text" => Ok(html_to_text(&body)),
        "markdown" | _ => Ok(html_to_markdown(&body)),
    }
}

fn html_to_text(html: &str) -> String {
    // Simple HTML to text conversion
    let mut text = String::new();
    let mut in_tag = false;
    let mut skip_script = false;
    let mut tag_name = String::new();

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_name.clear();
            continue;
        }
        if c == '>' {
            in_tag = false;
            let name = tag_name.to_lowercase();
            if name.starts_with("script") || name.starts_with("style") {
                skip_script = true;
            } else if name.starts_with("/script") || name.starts_with("/style") {
                skip_script = false;
            }
            if name == "br" || name == "br/" || name == "p" || name == "/p" || name == "div" || name == "/div" {
                text.push('\n');
            }
            continue;
        }
        if in_tag {
            if c.is_alphanumeric() || c == '/' {
                tag_name.push(c);
            }
            continue;
        }
        if !skip_script {
            text.push(c);
        }
    }

    // Clean up whitespace
    let lines: Vec<&str> = text.lines().collect();
    let cleaned: Vec<String> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    cleaned.join("\n")
}

fn html_to_markdown(html: &str) -> String {
    // Simple HTML to markdown conversion
    let mut md = String::new();
    let mut in_tag = false;
    let mut tag_name = String::new();
    let mut attrs = String::new();
    let mut skip_script = false;

    for c in html.chars() {
        if c == '<' {
            in_tag = true;
            tag_name.clear();
            attrs.clear();
            continue;
        }
        if c == '>' {
            in_tag = false;
            let full_tag = tag_name.to_lowercase();
            let name: String = full_tag.chars().take_while(|c| c.is_alphanumeric() || *c == '/').collect();

            if name.starts_with("script") || name.starts_with("style") {
                skip_script = true;
            } else if name.starts_with("/script") || name.starts_with("/style") {
                skip_script = false;
            }

            if skip_script { continue; }

            match name.as_str() {
                "h1" => md.push_str("\n# "),
                "/h1" => md.push('\n'),
                "h2" => md.push_str("\n## "),
                "/h2" => md.push('\n'),
                "h3" => md.push_str("\n### "),
                "/h3" => md.push('\n'),
                "h4" => md.push_str("\n#### "),
                "/h4" => md.push('\n'),
                "p" | "div" => md.push('\n'),
                "/p" | "/div" => md.push('\n'),
                "br" | "br/" => md.push('\n'),
                "strong" | "b" => md.push_str("**"),
                "/strong" | "/b" => md.push_str("**"),
                "em" | "i" => md.push('*'),
                "/em" | "/i" => md.push('*'),
                "code" => md.push('`'),
                "/code" => md.push('`'),
                "pre" => md.push_str("\n```\n"),
                "/pre" => md.push_str("\n```\n"),
                "ul" => md.push('\n'),
                "/ul" => md.push('\n'),
                "ol" => md.push('\n'),
                "/ol" => md.push('\n'),
                "li" => md.push_str("- "),
                "/li" => md.push('\n'),
                "a" => {
                    // Extract href from attrs
                    if let Some(href_start) = attrs.find("href=\"") {
                        let rest = &attrs[href_start + 7..];
                        if let Some(_end) = rest.find('"') {
                            let _href = &rest[.._end];
                            md.push('[');
                        }
                    }
                }
                "/a" => md.push_str("]"),
                _ => {}
            }
            continue;
        }
        if in_tag {
            if tag_name.is_empty() && c.is_alphabetic() || tag_name.starts_with('/') && c.is_alphabetic() {
                tag_name.push(c);
            } else if !tag_name.is_empty() {
                attrs.push(c);
            } else if c == '/' {
                tag_name.push(c);
            }
            continue;
        }
        if !skip_script {
            md.push(c);
        }
    }

    // Clean up
    let lines: Vec<&str> = md.lines().collect();
    let cleaned: Vec<String> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    cleaned.join("\n")
}
