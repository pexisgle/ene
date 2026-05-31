use chromiumoxide::page::Page;
use ene_tool_proto::ToolError;

pub async fn navigate(page: &Page, url: &str) -> Result<String, ToolError> {
    page.goto(url)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Navigation failed: {e}"),
        })?;
    let current_url = page
        .url()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to get URL: {e}"),
        })?
        .unwrap_or_else(|| url.to_string());
    let title = page
        .evaluate("document.title")
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to get title: {e}"),
        })?
        .into_value::<String>()
        .unwrap_or_default();
    let ready_state = page
        .evaluate("document.readyState")
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to get readyState: {e}"),
        })?
        .into_value::<String>()
        .unwrap_or_default();
    Ok(format!(
        "Navigation successful\nURL: {}\nTitle: {}\nReady State: {}",
        current_url, title, ready_state
    ))
}
