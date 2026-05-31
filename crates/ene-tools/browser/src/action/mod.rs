mod content;
mod interact;
mod navigate;
mod screenshot;

use chromiumoxide::page::Page;
use ene_tool_proto::ToolError;

pub async fn browser_exec(
    action: &str,
    page: &Page,
    url: Option<&str>,
    selector: Option<&str>,
    text: Option<&str>,
    wait_ms: Option<u64>,
    scroll_x: Option<i32>,
    scroll_y: Option<i32>,
    format: Option<&str>,
    extract: Option<&str>,
    trim: Option<bool>,
) -> Result<String, ToolError> {
    match action {
        "navigate" => {
            let url = url.ok_or_else(|| ToolError::InvalidArguments {
                message: "URL required for navigate".to_string(),
            })?;
            navigate::navigate(page, url).await
        }
        "click" => {
            let selector = selector.ok_or_else(|| ToolError::InvalidArguments {
                message: "Selector required for click".to_string(),
            })?;
            interact::click(page, selector).await
        }
        "type" => {
            let selector = selector.ok_or_else(|| ToolError::InvalidArguments {
                message: "Selector required for type".to_string(),
            })?;
            let text = text.ok_or_else(|| ToolError::InvalidArguments {
                message: "Text required for type".to_string(),
            })?;
            interact::type_text(page, selector, text).await
        }
        "wait" => interact::wait(wait_ms.unwrap_or(1000)).await,
        "screenshot" => screenshot::screenshot(page).await,
        "get_content" => {
            content::get_content(page, format, extract, trim.unwrap_or(true)).await
        }
        "scroll" => {
            let x = scroll_x.unwrap_or(0);
            let y = scroll_y.unwrap_or(0);
            content::scroll(page, x, y).await
        }
        "close" => Ok("Browser session closed.".to_string()),
        _ => Err(ToolError::InvalidArguments {
            message: format!("Unknown browser action: {}", action),
        }),
    }
}
