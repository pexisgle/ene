//! ブラウザ操作ツール (browser_tools)
//! Phase 3: Chromium と Chrome DevTools Protocol (CDP) を用いたブラウザ自動化

use crate::error::AiCoreError;
use super::definition::ToolDefinition;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "browser".to_string(),
        description: concat!(
            "Automates a Chromium browser via Chrome DevTools Protocol (CDP). ",
            "Supports navigation, clicking, text input, waiting for elements, taking screenshots, and extracting DOM content. ",
            "Use this when you need to interact with web pages programmatically."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "type", "wait", "screenshot", "get_content", "scroll"],
                    "description": "The browser action to perform"
                },
                "url": { "type": "string", "description": "URL for navigate action" },
                "selector": { "type": "string", "description": "CSS selector for click, type, wait actions" },
                "text": { "type": "string", "description": "Text to type into an element" },
                "wait_ms": { "type": "integer", "description": "Milliseconds to wait" },
                "scroll_x": { "type": "integer", "description": "Horizontal scroll amount" },
                "scroll_y": { "type": "integer", "description": "Vertical scroll amount" }
            },
            "required": ["action"]
        }),
    }
}

/// ブラウザ操作を実行
pub async fn browser_exec(
    action: &str,
    url: Option<&str>,
    selector: Option<&str>,
    text: Option<&str>,
    wait_ms: Option<u64>,
    scroll_x: Option<i32>,
    scroll_y: Option<i32>,
) -> Result<String, AiCoreError> {
    use chromiumoxide::browser::{Browser, BrowserConfig};
    use tokio_stream::StreamExt;

    let config = BrowserConfig::builder()
        .chrome_executable(std::path::PathBuf::from("chromium"))
        .build()
        .map_err(|e| AiCoreError::BrowserError(format!("Failed to build browser config: {e}")))?;

    let (browser, mut handler) = Browser::launch(config).await
        .map_err(|e| AiCoreError::BrowserError(format!("Failed to launch browser: {e}")))?;

    let _handle = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if h.is_err() {
                break;
            }
        }
    });

    let page = browser.new_page("about:blank").await
        .map_err(|e| AiCoreError::BrowserError(format!("Failed to create page: {e}")))?;

    let result = match action {
        "navigate" => {
            let url = url.ok_or_else(|| AiCoreError::BrowserError("URL required for navigate".to_string()))?;
            page.goto(url).await
                .map_err(|e| AiCoreError::BrowserError(format!("Navigation failed: {e}")))?;
            format!("Navigated to {}", url)
        }
        "click" => {
            let selector = selector.ok_or_else(|| AiCoreError::BrowserError("Selector required for click".to_string()))?;
            page.find_element(selector).await
                .map_err(|e| AiCoreError::BrowserError(format!("Element not found: {e}")))?
                .click().await
                .map_err(|e| AiCoreError::BrowserError(format!("Click failed: {e}")))?;
            format!("Clicked element: {}", selector)
        }
        "type" => {
            let selector = selector.ok_or_else(|| AiCoreError::BrowserError("Selector required for type".to_string()))?;
            let text = text.ok_or_else(|| AiCoreError::BrowserError("Text required for type".to_string()))?;
            page.find_element(selector).await
                .map_err(|e| AiCoreError::BrowserError(format!("Element not found: {e}")))?
                .type_str(text).await
                .map_err(|e| AiCoreError::BrowserError(format!("Type failed: {e}")))?;
            format!("Typed into element: {}", selector)
        }
        "wait" => {
            let ms = wait_ms.unwrap_or(1000);
            tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
            format!("Waited {} ms", ms)
        }
        "screenshot" => {
            let params = chromiumoxide::page::ScreenshotParams::default();
            let data = page.screenshot(params).await
                .map_err(|e| AiCoreError::BrowserError(format!("Screenshot failed: {e}")))?;
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
            let data_uri = format!("data:image/png;base64,{}", b64);
            return Ok(serde_json::json!({
                "type": "screenshot",
                "data": data_uri
            }).to_string());
        }
        "get_content" => {
            let content = page.content().await
                .map_err(|e| AiCoreError::BrowserError(format!("Failed to get content: {e}")))?;
            content
        }
        "scroll" => {
            let x = scroll_x.unwrap_or(0);
            let y = scroll_y.unwrap_or(0);
            let js = format!("window.scrollBy({}, {});", x, y);
            page.evaluate(js).await
                .map_err(|e| AiCoreError::BrowserError(format!("Scroll failed: {e}")))?;
            format!("Scrolled by ({}, {})", x, y)
        }
        _ => return Err(AiCoreError::BrowserError(format!("Unknown browser action: {}", action))),
    };

    Ok(result)
}
