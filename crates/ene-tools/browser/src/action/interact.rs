use chromiumoxide::page::Page;
use ene_tool_proto::ToolError;

pub async fn click(page: &Page, selector: &str) -> Result<String, ToolError> {
    page.find_element(selector)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Element not found: {e}"),
        })?
        .click()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Click failed: {e}"),
        })?;
    Ok(format!("Clicked element: {}", selector))
}

pub async fn type_text(page: &Page, selector: &str, text: &str) -> Result<String, ToolError> {
    page.find_element(selector)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Element not found: {e}"),
        })?
        .type_str(text)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Type failed: {e}"),
        })?;
    Ok(format!("Typed into element: {}", selector))
}

pub async fn wait(ms: u64) -> Result<String, ToolError> {
    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
    Ok(format!("Waited {} ms", ms))
}
