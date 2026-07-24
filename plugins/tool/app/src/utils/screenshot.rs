use base64::{Engine as _, engine::general_purpose};
use ene_tool_proto::ToolError;
use image::DynamicImage;
use std::io::Cursor;

/// Shared helper to encode `DynamicImage` into a Base64 PNG data URI.
pub fn encode_image_to_data_uri(image: &DynamicImage) -> Result<String, ToolError> {
    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| ToolError::execution_failed(format!("Failed to encode image to PNG: {e}")))?;
    let b64 = general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{b64}"))
}
