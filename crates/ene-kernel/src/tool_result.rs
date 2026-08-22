use crate::config::ToolOutputSettings;
use base64::Engine;
use serde_json::Value;

pub(crate) struct ExtractedImage {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Pull known image-base64 fields out of a tool JSON value.
pub(crate) fn extract_images(value: &mut Value) -> Vec<ExtractedImage> {
    let Some(obj) = value.as_object_mut() else {
        return Vec::new();
    };
    let mime_hint = obj
        .get("mime")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut out = Vec::new();
    for key in ["png_base64", "jpeg_base64", "webp_base64", "image_base64"] {
        let Some(Value::String(encoded)) = obj.remove(key) else {
            continue;
        };
        match base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes()) {
            Ok(bytes) if !bytes.is_empty() => {
                let mime = if mime_hint.starts_with("image/") {
                    mime_hint.clone()
                } else {
                    match key {
                        "jpeg_base64" => "image/jpeg",
                        "webp_base64" => "image/webp",
                        _ => "image/png",
                    }
                    .to_owned()
                };
                out.push(ExtractedImage { mime, bytes });
            }
            _ => {
                obj.insert(key.to_owned(), Value::String(encoded));
            }
        }
    }
    out
}

pub(crate) fn should_spill(len: usize, limits: &ToolOutputSettings) -> bool {
    let soft = usize::try_from(limits.soft_limit_bytes).unwrap_or(usize::MAX);
    len > soft.max(1)
}

pub(crate) fn spill_preview(text: &str, limits: &ToolOutputSettings) -> String {
    let hard = usize::try_from(limits.hard_limit_bytes).unwrap_or(usize::MAX);
    let preview_len = if text.len() > hard { 500 } else { 1_000 };
    truncate_chars(text, preview_len)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_images_strips_png_base64() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let mut value = json!({
            "mime": "image/png",
            "png_base64": encoded,
            "width": 1,
            "height": 1,
        });
        let images = extract_images(&mut value);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
        assert_eq!(images[0].bytes, png);
        assert!(value.get("png_base64").is_none());
        assert_eq!(value["width"], 1);
    }

    #[test]
    fn should_spill_uses_soft_limit_bytes() {
        let limits = ToolOutputSettings {
            soft_limit_bytes: 8,
            hard_limit_bytes: 32,
        };
        assert!(!should_spill(8, &limits));
        assert!(should_spill(9, &limits));
    }
}
