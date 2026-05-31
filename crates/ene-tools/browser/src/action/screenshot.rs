use chromiumoxide::page::Page;
use ene_tool_proto::ToolError;

pub async fn screenshot(page: &Page) -> Result<String, ToolError> {
    let params = chromiumoxide::page::ScreenshotParams::default();
    let data = page
        .screenshot(params)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Screenshot failed: {e}"),
        })?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
    let data_uri = format!("data:image/png;base64,{}", b64);
    Ok(serde_json::json!({ "type": "screenshot", "data": data_uri }).to_string())
}
