use std::path::Path;

use crate::error::{VrmError, VrmResult};

/// Load the encoded thumbnail referenced by a VRM 1.0 model, if it has one.
///
/// VRM stores the thumbnail as a glTF image index in
/// `VRMC_vrm.meta.thumbnailImage`. The returned bytes are still encoded (for
/// example, PNG or JPEG) so callers can choose the texture representation they
/// need. Only images embedded in a binary glTF buffer are read; the renderer's
/// supported VRM format is a self-contained `.glb`.
pub fn load_vrm_thumbnail(path: impl AsRef<Path>) -> VrmResult<Option<Vec<u8>>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| VrmError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|error| VrmError::Gltf(error.to_string()))?;
    if !gltf
        .document
        .extensions_used()
        .any(|extension| extension == "VRMC_vrm")
    {
        return Err(VrmError::NotVrm);
    }
    let Some(image_index) = thumbnail_image_index(&gltf.document) else {
        return Ok(None);
    };
    let Some(image) = gltf.images().find(|image| image.index() == image_index) else {
        return Ok(None);
    };
    let gltf::image::Source::View { view, .. } = image.source() else {
        return Ok(None);
    };
    if !matches!(view.buffer().source(), gltf::buffer::Source::Bin) {
        return Ok(None);
    }
    let Some(blob) = gltf.blob.as_deref() else {
        return Ok(None);
    };
    let Some(end) = view.offset().checked_add(view.length()) else {
        return Ok(None);
    };
    Ok(blob.get(view.offset()..end).map(<[u8]>::to_vec))
}

fn thumbnail_image_index(document: &gltf::Document) -> Option<usize> {
    document
        .extension_value("VRMC_vrm")
        .and_then(|extension| extension.get("meta"))
        .and_then(|meta| meta.get("thumbnailImage"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
}

#[cfg(test)]
mod tests {
    use super::load_vrm_thumbnail;

    #[test]
    fn vrm_without_thumbnail_returns_none() {
        let path = std::env::temp_dir().join(format!(
            "ene_vrm_thumbnail_missing_{}.vrm",
            std::process::id()
        ));
        std::fs::write(&path, crate::minimal::minimal_vrm_glb_bytes()).expect("write fixture");
        assert_eq!(load_vrm_thumbnail(&path).expect("read fixture"), None);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn embedded_thumbnail_bytes_are_returned() {
        let json = br#"{"asset":{"version":"2.0"},"extensionsUsed":["VRMC_vrm"],"extensions":{"VRMC_vrm":{"meta":{"thumbnailImage":0}}},"buffers":[{"byteLength":4}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":4}],"images":[{"bufferView":0,"mimeType":"image/png"}]}"#;
        let path = std::env::temp_dir().join(format!(
            "ene_vrm_thumbnail_embedded_{}.vrm",
            std::process::id()
        ));
        std::fs::write(&path, pack_glb(json, &[1, 2, 3, 4])).expect("write fixture");
        assert_eq!(
            load_vrm_thumbnail(&path).expect("read fixture"),
            Some(vec![1, 2, 3, 4])
        );
        std::fs::remove_file(path).ok();
    }

    fn pack_glb(json: &[u8], bin: &[u8]) -> Vec<u8> {
        let json_pad = (4 - (json.len() % 4)) % 4;
        let bin_pad = (4 - (bin.len() % 4)) % 4;
        let total_len = 12 + 8 + json.len() + json_pad + 8 + bin.len() + bin_pad;
        let mut glb = Vec::with_capacity(total_len);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&(total_len as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(json);
        glb.extend(std::iter::repeat_n(0_u8, json_pad));
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(bin);
        glb.extend(std::iter::repeat_n(0_u8, bin_pad));
        glb
    }
}
