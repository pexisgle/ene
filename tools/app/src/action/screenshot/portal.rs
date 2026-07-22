use ene_tool_proto::ToolError;
use image::DynamicImage;

pub(super) async fn capture_screen_portal(scale_percent: u32) -> Result<DynamicImage, ToolError> {
    use ashpd::desktop::screenshot::Screenshot;

    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
        .map_err(|e| ToolError::execution_failed(format!("Portal screenshot request failed: {e}")))?
        .response()
        .map_err(|e| {
            ToolError::execution_failed(format!("Portal screenshot response failed: {e}"))
        })?;

    let uri = response.uri();
    let path = uri.as_str().strip_prefix("file://").unwrap_or(uri.as_str());

    let image = image::open(path)
        .map_err(|e| ToolError::execution_failed(format!("Failed to open screenshot file: {e}")))?;

    let _ = std::fs::remove_file(path);

    Ok(crate::utils::image::resize_image(image, scale_percent))
}
