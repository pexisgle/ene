use chromiumoxide::page::Page;
use ene_tool_proto::ToolError;

pub async fn get_content(
    page: &Page,
    format: Option<&str>,
    extract: Option<&str>,
    trim: bool,
) -> Result<String, ToolError> {
    let format = format.unwrap_or("markdown");
    let extract = extract.unwrap_or("body");
    let html = page
        .content()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to get content: {e}"),
        })?;
    let extracted = match format {
        "html" => crate::extract::extract_html(&html, extract, trim),
        _ => crate::extract::extract_markdown(&html, extract, trim),
    };
    Ok(crate::extract::truncate_text(&extracted, 15000))
}

pub async fn scroll(page: &Page, x: i32, y: i32) -> Result<String, ToolError> {
    let js = format!("window.scrollBy({}, {});", x, y);
    page.evaluate(js)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Scroll failed: {e}"),
        })?;
    Ok(format!("Scrolled by ({}, {})", x, y))
}
